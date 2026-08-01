package api

import (
        "bufio"
        "fmt"
        "io"
        "net"
        "net/http"
        "strings"
        "time"

        "github.com/clouddesk-os/backend/internal/auth"
        "github.com/clouddesk-os/backend/internal/ide"
        "github.com/clouddesk-os/backend/pkg/models"
        "github.com/gin-gonic/gin"
)

// ideManager is set during initialization.
var ideManager *ide.Manager

// SetIDEManager injects the code-server manager into the API layer.
func SetIDEManager(m *ide.Manager) {
        ideManager = m
}

// handleIDEStatus returns the status of the user's code-server.
func (r *Router) handleIDEStatus(c *gin.Context) {
        claims, ok := getClaims(c)
        if !ok {
                c.JSON(http.StatusUnauthorized, models.APIError{Code: 401, Message: "unauthorized"})
                return
        }

        if ideManager == nil {
                c.JSON(http.StatusServiceUnavailable, models.APIError{
                        Code: 503, Message: "IDE service is not configured",
                })
                return
        }

        status := ideManager.GetStatus(claims.Username)

        c.JSON(http.StatusOK, gin.H{
                "status":   string(status),
                "username": claims.Username,
        })
}

// handleIDEStart starts the user's code-server instance.
func (r *Router) handleIDEStart(c *gin.Context) {
        claims, ok := getClaims(c)
        if !ok {
                c.JSON(http.StatusUnauthorized, models.APIError{Code: 401, Message: "unauthorized"})
                return
        }

        if ideManager == nil {
                c.JSON(http.StatusServiceUnavailable, models.APIError{
                        Code: 503, Message: "IDE service is not configured",
                })
                return
        }

        homeDir, err := getHomeDir(c)
        if err != nil {
                c.JSON(http.StatusInternalServerError, models.APIError{
                        Code: 500, Message: "failed to resolve home directory",
                })
                return
        }

        inst, err := ideManager.GetInstance(claims.Username, claims.UID, claims.GID, homeDir)
        if err != nil {
                c.JSON(http.StatusInternalServerError, models.APIError{
                        Code: 500, Message: "failed to get IDE instance", Details: err.Error(),
                })
                return
        }

        if err := ideManager.Start(inst); err != nil {
                c.JSON(http.StatusInternalServerError, models.APIError{
                        Code: 500, Message: "failed to start IDE", Details: err.Error(),
                })
                return
        }

        _ = r.auditLog(claims.UserID, claims.Username, "ide_start", "", c.ClientIP(), c.Request.UserAgent(), "")

        c.JSON(http.StatusOK, gin.H{
                "status":   string(ide.StatusRunning),
                "username": claims.Username,
        })
}

// handleIDEStop stops the user's code-server instance.
func (r *Router) handleIDEStop(c *gin.Context) {
        claims, ok := getClaims(c)
        if !ok {
                c.JSON(http.StatusUnauthorized, models.APIError{Code: 401, Message: "unauthorized"})
                return
        }

        if ideManager == nil {
                c.JSON(http.StatusServiceUnavailable, models.APIError{
                        Code: 503, Message: "IDE service is not configured",
                })
                return
        }

        inst, err := ideManager.GetInstance(claims.Username, 0, 0, "")
        if err != nil {
                c.JSON(http.StatusInternalServerError, models.APIError{
                        Code: 500, Message: "failed to get IDE instance", Details: err.Error(),
                })
                return
        }

        if err := ideManager.Stop(inst); err != nil {
                c.JSON(http.StatusInternalServerError, models.APIError{
                        Code: 500, Message: "failed to stop IDE", Details: err.Error(),
                })
                return
        }

        _ = r.auditLog(claims.UserID, claims.Username, "ide_stop", "", c.ClientIP(), c.Request.UserAgent(), "")

        c.JSON(http.StatusOK, gin.H{"status": string(ide.StatusStopped)})
}

// handleIDEProxy tunnels HTTP/WebSocket traffic to the user's code-server Unix socket.
func (r *Router) handleIDEProxy(c *gin.Context) {
        claims, ok := getClaims(c)
        if !ok {
                c.JSON(http.StatusUnauthorized, models.APIError{Code: 401, Message: "unauthorized"})
                return
        }

        if ideManager == nil {
                c.JSON(http.StatusServiceUnavailable, models.APIError{
                        Code: 503, Message: "IDE service is not configured",
                })
                return
        }

        // Connect to the code-server Unix socket.
        conn, err := ideManager.ProxyConn(claims.Username)
        if err != nil {
                c.JSON(http.StatusServiceUnavailable, models.APIError{
                        Code: 503, Message: "code-server is not running", Details: err.Error(),
                })
                return
        }
        defer conn.Close()

        // Hijack the HTTP connection for raw TCP proxying.
        hj, ok := c.Writer.(http.Hijacker)
        if !ok {
                c.JSON(http.StatusInternalServerError, models.APIError{
                        Code: 500, Message: "failed to hijack connection",
                })
                return
        }

        clientConn, clientBuf, err := hj.Hijack()
        if err != nil {
                return
        }
        defer clientConn.Close()

        // Detect WebSocket upgrade and handle accordingly.
        if isWebSocketUpgrade(c.Request) {
                proxyWebSocket(clientConn, clientBuf, conn)
        } else {
                proxyHTTP(clientConn, clientBuf, conn)
        }
}

// proxyHTTP proxies a regular HTTP connection to the Unix socket.
// Includes a read deadline on the buffered data to prevent hangs.
func proxyHTTP(clientConn net.Conn, clientBuf *bufio.ReadWriter, backendConn net.Conn) {
        done := make(chan struct{}, 2)

        go func() {
                // Set a read deadline on the buffered reader to prevent stalls
                // when the client already sent data before the hijack.
                clientConn.SetReadDeadline(time.Now().Add(5 * time.Second))
                io.Copy(backendConn, clientBuf.Reader)
                // Clear the deadline — bidirectional copy should not be time-limited.
                clientConn.SetReadDeadline(time.Time{})
                // Safely close write end if it's a Unix socket.
                if uc, ok := backendConn.(*net.UnixConn); ok {
                        uc.CloseWrite()
                }
                done <- struct{}{}
        }()

        go func() {
                io.Copy(clientConn, backendConn)
                done <- struct{}{}
        }()

        // Wait for BOTH directions to complete before returning.
        <-done
        <-done
}

// proxyWebSocket proxies a WebSocket connection to the Unix socket.
func proxyWebSocket(clientConn net.Conn, clientBuf *bufio.ReadWriter, backendConn net.Conn) {
        done := make(chan struct{}, 2)

        go func() {
                clientConn.SetReadDeadline(time.Now().Add(5 * time.Second))
                io.Copy(backendConn, clientBuf.Reader)
                clientConn.SetReadDeadline(time.Time{})
                // Safely close write end if it's a Unix socket.
                if uc, ok := backendConn.(*net.UnixConn); ok {
                        uc.CloseWrite()
                }
                done <- struct{}{}
        }()

        go func() {
                io.Copy(clientConn, backendConn)
                done <- struct{}{}
        }()

        // Wait for BOTH directions to complete before returning.
        <-done
        <-done
}

// isWebSocketUpgrade checks if the request is a WebSocket upgrade.
func isWebSocketUpgrade(req *http.Request) bool {
        return strings.EqualFold(req.Header.Get("Connection"), "Upgrade") &&
                strings.EqualFold(req.Header.Get("Upgrade"), "websocket")
}

// getHomeDir is a helper to resolve the user's home directory.
func getHomeDir(c *gin.Context) (string, error) {
        claims, ok := getClaims(c)
        if !ok {
                return "", fmt.Errorf("claims not found in request context")
        }
        return auth.HomePath(claims.Username)
}
