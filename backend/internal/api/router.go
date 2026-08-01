package api

import (
        "log"
        "net/http"
        "os"
        "path/filepath"
        "strings"
        "sync"
        "time"

        "github.com/clouddesk-os/backend/internal/auth"
        "github.com/gin-gonic/gin"
)

// loginAttempt tracks per-IP rate limiting state.
type loginAttempt struct {
        count    int
        windowStart time.Time
}

// Router sets up all API routes with middleware.
type Router struct {
        engine         *gin.Engine
        authenticator  *auth.PAMAuthenticator
        jwtManager     *auth.JWTManager
        allowedOrigins string
        frontendDist   string // Path to frontend build, empty if not configured.
        loginAttempts  map[string]*loginAttempt
        loginMu        sync.RWMutex
}

// NewRouter creates a new API router with all routes configured.
func NewRouter(a *auth.PAMAuthenticator, jm *auth.JWTManager, allowedOrigins string) *Router {
        gin.SetMode(gin.ReleaseMode)
        e := gin.New()

        // Detect frontend dist directory.
        frontendDist := ""
        if info, err := os.Stat("/opt/clouddesk/frontend/dist"); err == nil && info.IsDir() {
                frontendDist = "/opt/clouddesk/frontend/dist"
        }

        r := &Router{
                authenticator:  a,
                jwtManager:     jm,
                allowedOrigins: allowedOrigins,
                frontendDist:   frontendDist,
                loginAttempts:  make(map[string]*loginAttempt),
        }

        // Store engine separately since it's used in the struct.
        r.engine = e

        r.setupMiddleware()
        r.setupRoutes()

        return r
}

// Engine returns the underlying gin.Engine for the caller to attach to their HTTP server.
func (r *Router) Engine() *gin.Engine {
        return r.engine
}

func (r *Router) setupMiddleware() {
        // Recovery middleware catches panics.
        r.engine.Use(gin.Recovery())

        // Request logging middleware.
        r.engine.Use(r.loggingMiddleware())

        // CORS middleware.
        r.engine.Use(r.corsMiddleware())

        // Security headers.
        r.engine.Use(r.securityHeadersMiddleware())
}

// loginRateLimit middleware protects the login endpoint from brute-force attacks.
// Allows max 10 failed attempts per IP per 15-minute window.
func (r *Router) loginRateLimit() gin.HandlerFunc {
        return func(c *gin.Context) {
                clientIP := c.ClientIP()
                now := time.Now()

                r.loginMu.RLock()
                attempt, exists := r.loginAttempts[clientIP]
                r.loginMu.RUnlock()

                if exists {
                        r.loginMu.Lock()
                        // Re-read under write lock
                        attempt = r.loginAttempts[clientIP]
                        if now.Sub(attempt.windowStart) > 15*time.Minute {
                                // Window expired, reset
                                attempt.count = 0
                                attempt.windowStart = now
                        }
                        r.loginMu.Unlock()
                }

                r.loginMu.RLock()
                if attempt != nil && attempt.count >= 10 {
                        r.loginMu.RUnlock()
                        c.AbortWithStatusJSON(http.StatusTooManyRequests, gin.H{
                                "code":    429,
                                "message": "too many login attempts, try again later",
                        })
                        return
                }
                r.loginMu.RUnlock()

                c.Next()

                // Post-handler: if response status is 401, increment counter
                if c.Writer.Status() == http.StatusUnauthorized {
                        r.loginMu.Lock()
                        if _, exists := r.loginAttempts[clientIP]; !exists {
                                r.loginAttempts[clientIP] = &loginAttempt{windowStart: now}
                        }
                        r.loginAttempts[clientIP].count++
                        r.loginMu.Unlock()
                }
        }
}

func (r *Router) setupRoutes() {
        // Health check.
        r.engine.GET("/api/health", r.healthCheck)

        // Authentication routes (no JWT required).
        authGroup := r.engine.Group("/api/auth")
        authGroup.Use(r.loginRateLimit())
        {
                authGroup.POST("/login", r.handleLogin)
                authGroup.POST("/refresh", r.handleRefresh)
        }

        // API v1 — all routes below require JWT authentication.
        v1 := r.engine.Group("/api/v1")
        v1.Use(r.authMiddleware())
        {
                // File system operations.
                fs := v1.Group("/fs")
                {
                        fs.GET("/list", r.handleListDirectory)
                        fs.GET("/stat", r.handleStat)
                        fs.POST("/upload", r.handleUpload)
                        fs.GET("/download", r.handleDownload)
                        fs.DELETE("/delete", r.handleDelete)
                        fs.POST("/mkdir", r.handleMkdir)
                        fs.POST("/rename", r.handleRename)
                        fs.POST("/zip", r.handleZip)
                        fs.GET("/disk-usage", r.handleDiskUsage)
                }

                // IDE / Code-server proxy.
                ideGroup := v1.Group("/ide")
                {
                        ideGroup.GET("/status", r.handleIDEStatus)
                        ideGroup.POST("/start", r.handleIDEStart)
                        ideGroup.POST("/stop", r.handleIDEStop)
                        ideGroup.Any("/proxy/*path", r.handleIDEProxy)
                }

                // Terminal WebSocket (requires JWT auth).
                v1.GET("/terminal/ws", r.handleTerminal)

                // User info.
                v1.GET("/user/info", r.handleUserInfo)
        }

        // Serve frontend static files if available.
        if r.frontendDist != "" {
                r.engine.Static("/assets", filepath.Join(r.frontendDist, "assets"))
                r.engine.StaticFile("/favicon.svg", filepath.Join(r.frontendDist, "favicon.svg"))
        }

        // SPA fallback — serve index.html for all unknown routes.
        r.engine.NoRoute(r.serveFrontend)
}

// loggingMiddleware logs all requests.
func (r *Router) loggingMiddleware() gin.HandlerFunc {
        return func(c *gin.Context) {
                // Skip logging for static assets and health checks.
                if c.Request.URL.Path == "/api/health" || strings.HasPrefix(c.Request.URL.Path, "/assets/") {
                        c.Next()
                        return
                }

                c.Next()

                status := c.Writer.Status()
                clientIP := c.ClientIP()
                method := c.Request.Method
                path := c.Request.URL.Path
                responseSize := c.Writer.Size()

                log.Printf("[API] %s | %3d | %13d | %s | %s",
                        clientIP, status, responseSize, method, path,
                )
        }
}

// isOriginAllowed checks whether the given Origin header value is permitted.
// SECURITY: Wildcard "*" is NOT allowed when credentials are used (browser rejects it).
// If allowedOrigins is "*", we reflect the actual Origin back (dynamic allow-origin).
func (r *Router) isOriginAllowed(origin string) bool {
        if origin == "" {
                return false
        }
        if r.allowedOrigins == "*" {
                return true // Dynamic reflection — we send back the actual origin
        }
        for _, allowed := range strings.Split(r.allowedOrigins, ",") {
                if strings.TrimSpace(allowed) == origin {
                        return true
                }
        }
        return false
}

// corsMiddleware handles CORS.
// SECURITY: When credentials are enabled, browsers reject Access-Control-Allow-Origin: *.
// We use dynamic origin reflection instead: send back the actual request origin.
func (r *Router) corsMiddleware() gin.HandlerFunc {
        return func(c *gin.Context) {
                origin := c.Request.Header.Get("Origin")
                if origin != "" && r.isOriginAllowed(origin) {
                        c.Header("Access-Control-Allow-Origin", origin)
                        c.Header("Access-Control-Allow-Methods", "GET, POST, PUT, DELETE, OPTIONS, PATCH")
                        c.Header("Access-Control-Allow-Headers", "Authorization, Content-Type, X-Requested-With, Accept, Origin")
                        c.Header("Access-Control-Allow-Credentials", "true")
                        c.Header("Access-Control-Max-Age", "86400")
                        c.Header("Vary", "Origin")
                }

                if c.Request.Method == http.MethodOptions {
                        c.AbortWithStatus(http.StatusNoContent)
                        return
                }

                c.Next()
        }
}

// securityHeadersMiddleware adds security headers to all responses.
func (r *Router) securityHeadersMiddleware() gin.HandlerFunc {
        return func(c *gin.Context) {
                c.Header("X-Content-Type-Options", "nosniff")
                c.Header("X-Frame-Options", "DENY")
                c.Header("X-XSS-Protection", "1; mode=block")
                c.Header("Referrer-Policy", "strict-origin-when-cross-origin")
                c.Header("Content-Security-Policy", "default-src 'self'; script-src 'self' 'unsafe-inline' 'unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; connect-src 'self' ws: wss:; font-src 'self' data:; worker-src 'self' blob:")
                c.Header("Strict-Transport-Security", "max-age=31536000; includeSubDomains")
                c.Next()
        }
}

// authMiddleware validates JWT tokens from the Authorization header or cookie.
func (r *Router) authMiddleware() gin.HandlerFunc {
        return func(c *gin.Context) {
                tokenString := ""

                // Check Authorization header first.
                authHeader := c.GetHeader("Authorization")
                if strings.HasPrefix(authHeader, "Bearer ") {
                        tokenString = strings.TrimPrefix(authHeader, "Bearer ")
                }

                // Fall back to cookie.
                if tokenString == "" {
                        tokenString, _ = c.Cookie("clouddesk_token")
                }

                // SECURITY: WebSocket connections cannot send custom headers from the browser.
                // The terminal endpoint passes the token as a query parameter.
                if tokenString == "" {
                        tokenString = c.Query("token")
                }

                if tokenString == "" {
                        c.AbortWithStatusJSON(http.StatusUnauthorized, gin.H{
                                "code":    401,
                                "message": "authentication required",
                        })
                        return
                }

                claims, err := r.jwtManager.ValidateToken(tokenString)
                if err != nil {
                        // SECURITY: Do NOT leak token validation details to the client.
                        // Error details help attackers craft valid tokens.
                        log.Printf("[WARN] JWT validation failed from %s: %v", c.ClientIP(), err)
                        c.AbortWithStatusJSON(http.StatusUnauthorized, gin.H{
                                "code":    401,
                                "message": "invalid or expired token",
                        })
                        return
                }

                // Store claims in the Gin context for handlers to access.
                c.Set("claims", claims)
                c.Set("username", claims.Username)
                c.Set("os_uid", claims.UID)
                c.Set("os_gid", claims.GID)
                c.Set("user_id", claims.UserID)

                c.Next()
        }
}

// serveFrontend serves the React SPA's index.html for all unknown routes.
func (r *Router) serveFrontend(c *gin.Context) {
        if r.frontendDist != "" {
                indexPath := filepath.Join(r.frontendDist, "index.html")
                if _, err := os.Stat(indexPath); err == nil {
                        c.File(indexPath)
                        return
                }
        }

        c.JSON(http.StatusNotFound, gin.H{
                "code":    404,
                "message": "route not found",
        })
}

// getClaims is a helper to extract JWT claims from the Gin context.
func getClaims(c *gin.Context) (*auth.Claims, bool) {
        val, exists := c.Get("claims")
        if !exists {
                return nil, false
        }
        claims, ok := val.(*auth.Claims)
        return claims, ok
}
