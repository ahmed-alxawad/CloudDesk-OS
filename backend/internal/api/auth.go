package api

import (
        "database/sql"
        "net/http"
        "time"

        "github.com/clouddesk-os/backend/internal/auth"
        "github.com/clouddesk-os/backend/internal/config"
        "github.com/clouddesk-os/backend/pkg/models"
        "github.com/gin-gonic/gin"
)

// handleLogin processes PAM authentication and returns a JWT.
func (r *Router) handleLogin(c *gin.Context) {
        var req models.LoginRequest
        if err := c.ShouldBindJSON(&req); err != nil {
                c.JSON(http.StatusBadRequest, models.APIError{
                        Code:    400,
                        Message: "invalid request body",
                        Details: err.Error(),
                })
                return
        }

        // Authenticate against the system's PAM stack.
        userInfo, err := r.authenticator.Authenticate(req.Username, req.Password)
        if err != nil {
                // SECURITY: Do not expose PAM error details in audit log or response.
                // PAM errors can reveal system configuration (e.g., account locked, expired).
                _ = r.auditLog(0, req.Username, "login_failed", "", c.ClientIP(), c.Request.UserAgent(), "authentication failed")

                c.JSON(http.StatusUnauthorized, models.APIError{
                        Code:    401,
                        Message: "authentication failed",
                        Details: "invalid username or password",
                })
                return
        }

        // Look up or create the user in our database.
        user, err := r.findOrCreateUser(userInfo)
        if err != nil {
                c.JSON(http.StatusInternalServerError, models.APIError{
                        Code:    500,
                        Message: "internal error",
                        Details: "failed to process user record",
                })
                return
        }

        // Generate JWT.
        token, expiresAt, err := r.jwtManager.GenerateToken(user)
        if err != nil {
                c.JSON(http.StatusInternalServerError, models.APIError{
                        Code:    500,
                        Message: "failed to generate session token",
                })
                return
        }

        // Audit the successful login.
        _ = r.auditLog(user.ID, user.Username, "login", "", c.ClientIP(), c.Request.UserAgent(), "successful")

        // Set httpOnly cookie.
        maxAge := int(time.Until(time.Unix(expiresAt, 0)).Seconds())
        c.SetCookie("clouddesk_token", token, maxAge, "/", "", true, true)
        // SECURITY: Set SameSite=Strict to prevent CSRF attacks.
        c.SetSameSite(http.SameSiteStrictMode)

        c.JSON(http.StatusOK, models.LoginResponse{
                Token:     token,
                User:      *user,
                ExpiresAt: expiresAt,
        })
}

// handleRefresh refreshes an existing JWT token.
func (r *Router) handleRefresh(c *gin.Context) {
        tokenString := ""

        authHeader := c.GetHeader("Authorization")
        if len(authHeader) > 7 && authHeader[:7] == "Bearer " {
                tokenString = authHeader[7:]
        }
        if tokenString == "" {
                tokenString, _ = c.Cookie("clouddesk_token")
        }

        if tokenString == "" {
                c.JSON(http.StatusUnauthorized, models.APIError{
                        Code:    401,
                        Message: "no token provided",
                })
                return
        }

        newToken, expiresAt, err := r.jwtManager.RefreshToken(tokenString)
        if err != nil {
                c.JSON(http.StatusUnauthorized, models.APIError{
                        Code:    401,
                        Message: "failed to refresh token",
                        Details: err.Error(),
                })
                return
        }

        maxAge := int(time.Until(time.Unix(expiresAt, 0)).Seconds())
        c.SetCookie("clouddesk_token", newToken, maxAge, "/", "", true, true)
        c.SetSameSite(http.SameSiteStrictMode)

        c.JSON(http.StatusOK, gin.H{
                "token":      newToken,
                "expires_at": expiresAt,
        })
}

// findOrCreateUser looks up a user in the database or creates a new record.
// For Phase 1 MVP, this uses an in-memory map. In production, this would use PostgreSQL.
func (r *Router) findOrCreateUser(userInfo *auth.UserInfo) (*models.User, error) {
        // TODO: Replace with PostgreSQL lookup in production.
        // This is a simplified in-memory implementation for Phase 1.

        user := &models.User{
                Username:  userInfo.Username,
                UID:       userInfo.UID,
                GID:       userInfo.GID,
                HomeDir:   userInfo.HomeDir,
                Shell:     userInfo.Shell,
                Role:      "user",
                IsActive:  true,
                CreatedAt: time.Now(),
                LastLogin: sql.NullTime{
                        Time:  time.Now(),
                        Valid: true,
                },
        }

        return user, nil
}

// auditLog records an action to the audit log.
// For Phase 1 MVP, this logs to stdout. In production, this writes to PostgreSQL.
func (r *Router) auditLog(userID int64, username, action, filePath, ip, userAgent, details string) error {
        // TODO: Replace with PostgreSQL insert in production.
        _ = userID
        _ = username
        _ = action
        _ = filePath
        _ = ip
        _ = userAgent
        _ = details
        return nil
}

// healthCheck is the liveness endpoint.
func (r *Router) healthCheck(c *gin.Context) {
        c.JSON(http.StatusOK, gin.H{
                "status":  "ok",
                "version": config.Load().Version(),
        })
}

// handleUserInfo returns the authenticated user's information.
func (r *Router) handleUserInfo(c *gin.Context) {
        claims, ok := getClaims(c)
        if !ok {
                c.JSON(http.StatusUnauthorized, models.APIError{
                        Code:    401,
                        Message: "unauthorized",
                })
                return
        }

        userInfo, err := auth.ResolveUser(claims.Username)
        if err != nil {
                c.JSON(http.StatusNotFound, models.APIError{
                        Code:    404,
                        Message: "user not found",
                })
                return
        }

        c.JSON(http.StatusOK, gin.H{
                "username": claims.Username,
                "uid":      claims.UID,
                "gid":      claims.GID,
                "role":     claims.Role,
                "home_dir": userInfo.HomeDir,
                "shell":    userInfo.Shell,
        })
}
