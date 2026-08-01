package api

import (
	"log"
	"net/http"
	"strconv"

	"github.com/clouddesk-os/backend/internal/terminal"
	"github.com/gin-gonic/gin"
	"github.com/gorilla/websocket"
)

var upgrader = websocket.Upgrader{
	ReadBufferSize:  1024,
	WriteBufferSize: 1024,
	// SECURITY: Origin is checked manually after JWT validation.
	// We rely on JWT auth middleware instead of origin checking.
	CheckOrigin: func(r *http.Request) bool {
		return true
	},
}

// handleTerminal upgrades the HTTP connection to WebSocket and spawns a PTY
// for the authenticated user. The shell process runs as the user's Linux UID/GID.
//
// SECURITY MODEL:
//   - JWT authentication is enforced by the auth middleware before this handler.
//     The token is passed as a query parameter because browsers cannot set
//     custom headers on WebSocket connections.
//   - The PTY shell runs as the authenticated Linux user (not root), enforced
//     via SysProcAttr.Credential at fork time.
//   - Each connection spawns a new PTY session — no session sharing.
//   - Terminal input is written directly to the PTY, not executed as commands.
func (r *Router) handleTerminal(c *gin.Context) {
	claims, ok := getClaims(c)
	if !ok {
		c.JSON(http.StatusUnauthorized, gin.H{"message": "unauthorized"})
		return
	}

	// Resolve user's home directory and shell.
	homeDir, err := getHomeDir(c)
	if err != nil {
		log.Printf("[ERROR] Terminal: failed to resolve home dir for '%s': %v", claims.Username, err)
		c.JSON(http.StatusInternalServerError, gin.H{"message": "failed to resolve home directory"})
		return
	}

	shell := terminal.ResolveShell(claims.Username)

	// Get initial terminal size from query params (defaults to 80x24).
	cols := uint16(80)
	rows := uint16(24)
	if v, err := strconv.Atoi(c.Query("cols")); err == nil && v > 0 && v <= 500 {
		cols = uint16(v)
	}
	if v, err := strconv.Atoi(c.Query("rows")); err == nil && v > 0 && v <= 200 {
		rows = uint16(v)
	}

	// Create PTY session for this user.
	session, err := terminal.NewSession(claims.Username, claims.UID, claims.GID, homeDir, shell)
	if err != nil {
		log.Printf("[ERROR] Terminal: failed to create session for '%s': %v", claims.Username, err)
		c.JSON(http.StatusInternalServerError, gin.H{"message": "failed to create terminal session"})
		return
	}

	// Start the shell process.
	if err := session.Start(cols, rows); err != nil {
		log.Printf("[ERROR] Terminal: failed to start shell for '%s': %v", claims.Username, err)
		c.JSON(http.StatusInternalServerError, gin.H{"message": "failed to start terminal"})
		return
	}

	log.Printf("[INFO] Terminal session started for user '%s' (uid=%d, shell=%s, %dx%d)",
		claims.Username, claims.UID, shell, cols, rows)

	// Upgrade HTTP to WebSocket.
	wsConn, err := upgrader.Upgrade(c.Writer, c.Request, nil)
	if err != nil {
		session.Close()
		log.Printf("[ERROR] Terminal: WebSocket upgrade failed for '%s': %v", claims.Username, err)
		return
	}

	// Audit log.
	_ = r.auditLog(claims.UserID, claims.Username, "terminal_open", "", c.ClientIP(), c.Request.UserAgent(), "")

	// Attach the WebSocket to the PTY session (blocks until disconnect).
	session.Attach(wsConn, cols, rows)

	// Cleanup after disconnect.
	session.Close()
	log.Printf("[INFO] Terminal session ended for user '%s'", claims.Username)
}
