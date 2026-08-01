package main

import (
	"context"
	"fmt"
	"log"
	"net"
	"net/http"
	"os"
	"os/signal"
	"path/filepath"
	"syscall"
	"time"

	"github.com/clouddesk-os/backend/internal/api"
	"github.com/clouddesk-os/backend/internal/auth"
	"github.com/clouddesk-os/backend/internal/config"
	"github.com/clouddesk-os/backend/internal/ide"
)

func main() {
	// Load configuration (parses all flags including -jwt-secret-file, -master-key-file).
	cfg := config.Load()

	// Banner.
	fmt.Println(`
  ██████╗ ██████╗ ███╗   ███╗███╗   ███╗ ██████╗ ███╗   ██╗███████╗
 ██╔════╝██╔═══██╗████╗ ████║████╗ ████║██╔═══██╗████╗  ██║██╔════╝
 ██║     ██║   ██║██╔████╔██║██╔████╔██║██║   ██║██╔██╗ ██║█████╗  
 ██║     ██║   ██║██║╚██╔╝██║██║╚██╔╝██║██║   ██║██║╚██╗██║██╔══╝  
 ╚██████╗╚██████╔╝██║ ╚═╝ ██║██║ ╚═╝ ██║╚██████╔╝██║ ╚████║███████╗
  ╚═════╝ ╚═════╝ ╚═╝     ╚═╝╚═╝     ╚═╝ ╚═════╝ ╚═╝  ╚═══╝╚══════╝

  CloudDesk OS v` + cfg.Version() + ` — Phase 1 MVP
  Transform your Linux server into a browser-based workspace.
`)

	// Verify running as root (required for PAM + privilege dropping).
	auth.RequireRoot()

	// Initialize PAM authenticator.
	pamAuth := auth.NewPAMAuthenticator("clouddesk")

	// Initialize JWT manager.
	jwtMgr := auth.NewJWTManager(cfg.JWT.Secret, cfg.JWT.ExpirationHours)

	// Initialize code-server IDE manager (optional — can be nil if code-server not installed).
	var ideMgr *ide.Manager
	if _, err := os.Stat(cfg.CodeServer.BinPath); err == nil {
		ideMgr, err = ide.NewManager(
			cfg.CodeServer.BinPath,
			cfg.CodeServer.SocketDir,
			cfg.CodeServer.DataDir,
			cfg.CodeServer.ExtraArgs,
		)
		if err != nil {
			log.Printf("[WARN] Failed to initialize IDE manager: %v", err)
			log.Printf("[WARN] IDE features will be disabled. Install code-server to enable them.")
			ideMgr = nil
		} else {
			log.Printf("[INFO] IDE manager initialized (socket dir: %s)", cfg.CodeServer.SocketDir)
			api.SetIDEManager(ideMgr)
		}
	} else {
		log.Printf("[INFO] code-server not found at '%s' — IDE features disabled", cfg.CodeServer.BinPath)
	}

	// Set up API router.
	router := api.NewRouter(pamAuth, jwtMgr, cfg.Security.AllowedOrigins)
	engine := router.Engine()

	// Serve frontend static files if the dist directory exists.
	frontendDist := "/opt/clouddesk/frontend/dist"
	if info, err := os.Stat(frontendDist); err == nil && info.IsDir() {
		engine.Static("/assets", frontendDist+"/assets")
		engine.StaticFile("/favicon.svg", frontendDist+"/favicon.svg")
		log.Printf("[INFO] Frontend static files served from %s", frontendDist)
	}

	// Create HTTP server.
	addr := fmt.Sprintf("%s:%d", cfg.Server.Host, cfg.Server.Port)
	srv := &http.Server{
		Addr:              addr,
		Handler:           engine,
		ReadHeaderTimeout: 10 * time.Second,
		ReadTimeout:       30 * time.Second,
		WriteTimeout:      300 * time.Second, // Long timeout for large file uploads.
		IdleTimeout:       120 * time.Second,
		MaxHeaderBytes:    1 << 20, // 1 MB
	}

	// Set up graceful shutdown.
	quit := make(chan os.Signal, 1)
	signal.Notify(quit, syscall.SIGINT, syscall.SIGTERM, syscall.SIGHUP)

	// Start HTTP listener.
	listener, err := net.Listen("tcp", addr)
	if err != nil {
		log.Fatalf("[FATAL] Failed to bind to %s: %v", addr, err)
	}

	// In production behind nginx, we also listen on a Unix socket for internal routing.
	unixSock := "/var/run/clouddesk/server.sock"
	if err := os.MkdirAll(filepath.Dir(unixSock), 0755); err != nil {
		log.Printf("[WARN] Failed to create Unix socket directory %s: %v", filepath.Dir(unixSock), err)
	}
	unixListener, err := net.Listen("unix", unixSock)
	if err != nil {
		log.Printf("[WARN] Failed to create Unix socket at %s: %v", unixSock, err)
	} else {
		go func() {
			log.Printf("[INFO] Listening on Unix socket: %s", unixSock)
			if err := srv.Serve(unixListener); err != nil && err != http.ErrServerClosed {
				log.Printf("[ERROR] Unix socket server error: %v", err)
			}
		}()
	}

	// Start serving HTTP.
	go func() {
		log.Printf("[INFO] CloudDesk OS API listening on http://%s", addr)
		log.Printf("[INFO] PAM service: clouddesk")
		log.Printf("[INFO] JWT expiry: %d hours", cfg.JWT.ExpirationHours)
		log.Printf("[INFO] Home base: %s", cfg.VFS.HomeBasePath)
		if err := srv.Serve(listener); err != nil && err != http.ErrServerClosed {
			log.Fatalf("[FATAL] Server error: %v", err)
		}
	}()

	// Wait for shutdown signal.
	<-quit
	log.Println("[INFO] Shutdown signal received, cleaning up...")

	// Stop all code-server instances.
	if ideMgr != nil {
		instances := ideMgr.ListInstances()
		for username, status := range instances {
			if status == ide.StatusRunning || status == ide.StatusStarting {
				inst, _ := ideMgr.GetInstance(username, 0, 0, "")
				if inst != nil {
					log.Printf("[INFO] Stopping code-server for user '%s'", username)
					_ = ideMgr.Stop(inst)
				}
			}
		}
	}

	// Graceful HTTP shutdown with timeout.
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	if err := srv.Shutdown(ctx); err != nil {
		log.Printf("[ERROR] Server shutdown error: %v", err)
	}

	log.Println("[INFO] CloudDesk OS has been shut down gracefully.")
}
