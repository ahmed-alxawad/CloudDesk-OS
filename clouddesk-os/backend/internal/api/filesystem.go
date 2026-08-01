package api

import (
        "fmt"
        "io"
        "log"
        "net/http"
        "os"
        "path/filepath"
        "strconv"
        "strings"

        "github.com/clouddesk-os/backend/internal/auth"
        "github.com/clouddesk-os/backend/internal/vfs"
        "github.com/clouddesk-os/backend/pkg/models"
        "github.com/gin-gonic/gin"
)

// validatePath checks for null bytes and excessive length.
func validatePath(path string) (string, bool) {
        if strings.ContainsRune(path, '\x00') {
                return "path contains invalid characters", false
        }
        if len(path) > 4096 {
                return "path too long", false
        }
        return "", true
}

// handleListDirectory returns the contents of a directory.
func (r *Router) handleListDirectory(c *gin.Context) {
        claims, ok := getClaims(c)
        if !ok {
                c.JSON(http.StatusUnauthorized, models.APIError{Code: 401, Message: "unauthorized"})
                return
        }

        path := c.Query("path")
        if path == "" {
                path = "~"
        }

        // Resolve the home directory.
        homeDir, err := auth.HomePath(claims.Username)
        if err != nil {
                c.JSON(http.StatusInternalServerError, models.APIError{
                        Code: 500, Message: "failed to resolve home directory", Details: err.Error(),
                })
                return
        }

        if path == "~" || path == claims.Username {
                path = homeDir
        } else if strings.HasPrefix(path, "~/") {
                path = filepath.Join(homeDir, path[2:])
        }

        if msg, ok := validatePath(path); !ok {
                c.JSON(http.StatusBadRequest, models.APIError{Code: 400, Message: msg})
                return
        }

        fs := vfs.NewLocalVFS(claims.UID, claims.GID, homeDir)
        entries, err := fs.List(path)
        if err != nil {
                status := http.StatusInternalServerError
                if strings.Contains(err.Error(), "permission denied") {
                        status = http.StatusForbidden
                } else if strings.Contains(err.Error(), "not found") {
                        status = http.StatusNotFound
                }
                c.JSON(status, models.APIError{
                        Code: status, Message: "failed to list directory", Details: err.Error(),
                })
                return
        }

        c.JSON(http.StatusOK, gin.H{
                "path":    path,
                "entries": entries,
        })
}

// handleStat returns file metadata.
func (r *Router) handleStat(c *gin.Context) {
        claims, ok := getClaims(c)
        if !ok {
                c.JSON(http.StatusUnauthorized, models.APIError{Code: 401, Message: "unauthorized"})
                return
        }

        path := c.Query("path")
        if path == "" {
                c.JSON(http.StatusBadRequest, models.APIError{Code: 400, Message: "path parameter required"})
                return
        }
        if msg, ok := validatePath(path); !ok {
                c.JSON(http.StatusBadRequest, models.APIError{Code: 400, Message: msg})
                return
        }

        homeDir, err := auth.HomePath(claims.Username)
        if err != nil {
                c.JSON(http.StatusInternalServerError, models.APIError{
                        Code: 500, Message: "failed to resolve home directory",
                })
                return
        }

        if strings.HasPrefix(path, "~/") {
                path = filepath.Join(homeDir, path[2:])
        }

        fs := vfs.NewLocalVFS(claims.UID, claims.GID, homeDir)
        info, err := fs.Stat(path)
        if err != nil {
                c.JSON(http.StatusNotFound, models.APIError{
                        Code: 404, Message: "file not found", Details: err.Error(),
                })
                return
        }

        c.JSON(http.StatusOK, info)
}

// handleUpload processes a multipart file upload.
func (r *Router) handleUpload(c *gin.Context) {
        claims, ok := getClaims(c)
        if !ok {
                c.JSON(http.StatusUnauthorized, models.APIError{Code: 401, Message: "unauthorized"})
                return
        }

        homeDir, err := auth.HomePath(claims.Username)
        if err != nil {
                c.JSON(http.StatusInternalServerError, models.APIError{
                        Code: 500, Message: "failed to resolve home directory",
                })
                return
        }

        targetPath := c.PostForm("path")
        if targetPath == "" {
                targetPath = homeDir
        } else if strings.HasPrefix(targetPath, "~/") {
                targetPath = filepath.Join(homeDir, targetPath[2:])
        }

        file, header, err := c.Request.FormFile("file")
        if err != nil {
                c.JSON(http.StatusBadRequest, models.APIError{
                        Code: 400, Message: "failed to read uploaded file", Details: err.Error(),
                })
                return
        }
        defer file.Close()

        fs := vfs.NewLocalVFS(claims.UID, claims.GID, homeDir)

        // Limit upload size to 10GB (enforced by middleware in production).
        maxSize := int64(10 * 1024 * 1024 * 1024)
        if header.Size > maxSize {
                c.JSON(http.StatusRequestEntityTooLarge, models.APIError{
                        Code: 413, Message: fmt.Sprintf("file too large (max %d GB)", maxSize/(1024*1024*1024)),
                })
                return
        }

        destPath := filepath.Join(targetPath, header.Filename)
        resp, err := fs.Write(destPath, file, header.Size)
        if err != nil {
                status := http.StatusInternalServerError
                if strings.Contains(err.Error(), "permission denied") {
                        status = http.StatusForbidden
                }
                c.JSON(status, models.APIError{
                        Code: status, Message: "upload failed", Details: err.Error(),
                })
                return
        }

        // Audit log.
        _ = r.auditLog(claims.UserID, claims.Username, "upload", resp.Path, c.ClientIP(), c.Request.UserAgent(), resp.Name)

        c.JSON(http.StatusCreated, resp)
}

// handleDownload streams a file to the client.
func (r *Router) handleDownload(c *gin.Context) {
        claims, ok := getClaims(c)
        if !ok {
                c.JSON(http.StatusUnauthorized, models.APIError{Code: 401, Message: "unauthorized"})
                return
        }

        path := c.Query("path")
        if path == "" {
                c.JSON(http.StatusBadRequest, models.APIError{Code: 400, Message: "path parameter required"})
                return
        }
        if msg, ok := validatePath(path); !ok {
                c.JSON(http.StatusBadRequest, models.APIError{Code: 400, Message: msg})
                return
        }

        homeDir, err := auth.HomePath(claims.Username)
        if err != nil {
                c.JSON(http.StatusInternalServerError, models.APIError{
                        Code: 500, Message: "failed to resolve home directory",
                })
                return
        }

        if strings.HasPrefix(path, "~/") {
                path = filepath.Join(homeDir, path[2:])
        }

        fs := vfs.NewLocalVFS(claims.UID, claims.GID, homeDir)
        reader, info, err := fs.Read(path)
        if err != nil {
                status := http.StatusInternalServerError
                if strings.Contains(err.Error(), "not found") {
                        status = http.StatusNotFound
                } else if strings.Contains(err.Error(), "permission denied") {
                        status = http.StatusForbidden
                }
                c.JSON(status, models.APIError{
                        Code: status, Message: "download failed", Details: err.Error(),
                })
                return
        }
        defer reader.Close()

        // Set content headers with sanitized filename.
        c.Header("Content-Disposition", fmt.Sprintf("attachment; filename=%s", strconv.QuoteToASCII(info.Name)))
        c.Header("Content-Type", info.MimeType)
        c.Header("Content-Length", strconv.FormatInt(info.Size, 10))
        c.Header("Cache-Control", "no-cache")

        c.Status(http.StatusOK)
        if _, err := io.Copy(c.Writer, reader); err != nil {
                log.Printf("[ERROR] download stream error for path %s: %v", path, err)
        }

        // Audit log.
        _ = r.auditLog(claims.UserID, claims.Username, "download", path, c.ClientIP(), c.Request.UserAgent(), info.Name)
}

// handleDelete removes a file or directory.
func (r *Router) handleDelete(c *gin.Context) {
        claims, ok := getClaims(c)
        if !ok {
                c.JSON(http.StatusUnauthorized, models.APIError{Code: 401, Message: "unauthorized"})
                return
        }

        path := c.Query("path")
        if path == "" {
                c.JSON(http.StatusBadRequest, models.APIError{Code: 400, Message: "path parameter required"})
                return
        }
        if msg, ok := validatePath(path); !ok {
                c.JSON(http.StatusBadRequest, models.APIError{Code: 400, Message: msg})
                return
        }

        homeDir, err := auth.HomePath(claims.Username)
        if err != nil {
                c.JSON(http.StatusInternalServerError, models.APIError{
                        Code: 500, Message: "failed to resolve home directory",
                })
                return
        }

        if strings.HasPrefix(path, "~/") {
                path = filepath.Join(homeDir, path[2:])
        }

        fs := vfs.NewLocalVFS(claims.UID, claims.GID, homeDir)
        if err := fs.Delete(path); err != nil {
                status := http.StatusInternalServerError
                if strings.Contains(err.Error(), "permission denied") {
                        status = http.StatusForbidden
                } else if strings.Contains(err.Error(), "not found") {
                        status = http.StatusNotFound
                }
                c.JSON(status, models.APIError{
                        Code: status, Message: "delete failed", Details: err.Error(),
                })
                return
        }

        _ = r.auditLog(claims.UserID, claims.Username, "delete", path, c.ClientIP(), c.Request.UserAgent(), "")

        c.JSON(http.StatusOK, gin.H{"message": "deleted"})
}

// handleMkdir creates a new directory.
func (r *Router) handleMkdir(c *gin.Context) {
        claims, ok := getClaims(c)
        if !ok {
                c.JSON(http.StatusUnauthorized, models.APIError{Code: 401, Message: "unauthorized"})
                return
        }

        var req struct {
                Path string `json:"path" binding:"required"`
        }
        if err := c.ShouldBindJSON(&req); err != nil {
                c.JSON(http.StatusBadRequest, models.APIError{Code: 400, Message: "invalid request"})
                return
        }
        if msg, ok := validatePath(req.Path); !ok {
                c.JSON(http.StatusBadRequest, models.APIError{Code: 400, Message: msg})
                return
        }

        homeDir, err := auth.HomePath(claims.Username)
        if err != nil {
                c.JSON(http.StatusInternalServerError, models.APIError{
                        Code: 500, Message: "failed to resolve home directory",
                })
                return
        }

        if strings.HasPrefix(req.Path, "~/") {
                req.Path = filepath.Join(homeDir, req.Path[2:])
        }

        fs := vfs.NewLocalVFS(claims.UID, claims.GID, homeDir)
        if err := fs.Mkdir(req.Path, 0755); err != nil {
                c.JSON(http.StatusInternalServerError, models.APIError{
                        Code: 500, Message: "mkdir failed", Details: err.Error(),
                })
                return
        }

        _ = r.auditLog(claims.UserID, claims.Username, "mkdir", req.Path, c.ClientIP(), c.Request.UserAgent(), "")

        c.JSON(http.StatusCreated, gin.H{"message": "directory created"})
}

// handleRename moves/renames a file.
func (r *Router) handleRename(c *gin.Context) {
        claims, ok := getClaims(c)
        if !ok {
                c.JSON(http.StatusUnauthorized, models.APIError{Code: 401, Message: "unauthorized"})
                return
        }

        var req struct {
                OldPath string `json:"old_path" binding:"required"`
                NewPath string `json:"new_path" binding:"required"`
        }
        if err := c.ShouldBindJSON(&req); err != nil {
                c.JSON(http.StatusBadRequest, models.APIError{Code: 400, Message: "invalid request"})
                return
        }
        if msg, ok := validatePath(req.OldPath); !ok {
                c.JSON(http.StatusBadRequest, models.APIError{Code: 400, Message: msg})
                return
        }
        if msg, ok := validatePath(req.NewPath); !ok {
                c.JSON(http.StatusBadRequest, models.APIError{Code: 400, Message: msg})
                return
        }

        homeDir, err := auth.HomePath(claims.Username)
        if err != nil {
                c.JSON(http.StatusInternalServerError, models.APIError{
                        Code: 500, Message: "failed to resolve home directory",
                })
                return
        }

        if strings.HasPrefix(req.OldPath, "~/") {
                req.OldPath = filepath.Join(homeDir, req.OldPath[2:])
        }
        if strings.HasPrefix(req.NewPath, "~/") {
                req.NewPath = filepath.Join(homeDir, req.NewPath[2:])
        }

        fs := vfs.NewLocalVFS(claims.UID, claims.GID, homeDir)
        if err := fs.Rename(req.OldPath, req.NewPath); err != nil {
                c.JSON(http.StatusInternalServerError, models.APIError{
                        Code: 500, Message: "rename failed", Details: err.Error(),
                })
                return
        }

        _ = r.auditLog(claims.UserID, claims.Username, "rename", req.NewPath, c.ClientIP(), c.Request.UserAgent(), req.OldPath)

        c.JSON(http.StatusOK, gin.H{"message": "renamed"})
}

// handleZip creates a zip archive of the specified entries.
func (r *Router) handleZip(c *gin.Context) {
        claims, ok := getClaims(c)
        if !ok {
                c.JSON(http.StatusUnauthorized, models.APIError{Code: 401, Message: "unauthorized"})
                return
        }

        var req struct {
                Entries   []string `json:"entries" binding:"required,min=1"`
                TargetDir string   `json:"target_dir"`
        }
        if err := c.ShouldBindJSON(&req); err != nil {
                c.JSON(http.StatusBadRequest, models.APIError{Code: 400, Message: "invalid request"})
                return
        }

        homeDir, err := auth.HomePath(claims.Username)
        if err != nil {
                c.JSON(http.StatusInternalServerError, models.APIError{
                        Code: 500, Message: "failed to resolve home directory",
                })
                return
        }

        targetDir := homeDir
        if req.TargetDir != "" {
                if strings.HasPrefix(req.TargetDir, "~/") {
                        targetDir = filepath.Join(homeDir, req.TargetDir[2:])
                } else {
                        targetDir = req.TargetDir
                }
        }

        zipName := fmt.Sprintf("archive_%s.zip", filepath.Base(req.Entries[0]))
        targetPath := filepath.Join(targetDir, zipName)

        fs := vfs.NewLocalVFS(claims.UID, claims.GID, homeDir)
        resultPath, err := fs.ZipArchive(targetPath, req.Entries)
        if err != nil {
                c.JSON(http.StatusInternalServerError, models.APIError{
                        Code: 500, Message: "zip creation failed", Details: err.Error(),
                })
                return
        }

        // Stream the zip back to the client.
        // SECURITY: Open the file as the target user to verify read permissions.
        fsZip := vfs.NewLocalVFS(claims.UID, claims.GID, homeDir)
        var zipReader io.ReadCloser
        var zipStat os.FileInfo
        err = fsZip.AsUser(func() error {
                f, err := os.Open(resultPath)
                if err != nil {
                        return fmt.Errorf("failed to open archive: %w", err)
                }
                zipReader = f
                zipStat, err = f.Stat()
                return err
        })
        if err != nil {
                c.JSON(http.StatusInternalServerError, models.APIError{
                        Code: 500, Message: "failed to open archive",
                })
                return
        }
        defer zipReader.Close()

        c.Header("Content-Disposition", fmt.Sprintf("attachment; filename=%s", strconv.QuoteToASCII(zipName)))
        c.Header("Content-Type", "application/zip")
        c.Header("Content-Length", strconv.FormatInt(zipStat.Size(), 10))

        if _, err := io.Copy(c.Writer, zipReader); err != nil {
                log.Printf("[ERROR] zip stream error: %v", err)
        }

        _ = r.auditLog(claims.UserID, claims.Username, "zip", resultPath, c.ClientIP(), c.Request.UserAgent(), "")
}

// handleDiskUsage returns disk usage information.
func (r *Router) handleDiskUsage(c *gin.Context) {
        claims, ok := getClaims(c)
        if !ok {
                c.JSON(http.StatusUnauthorized, models.APIError{Code: 401, Message: "unauthorized"})
                return
        }

        path := c.Query("path")
        if path == "" {
                path = "~/"
        }

        homeDir, err := auth.HomePath(claims.Username)
        if err != nil {
                c.JSON(http.StatusInternalServerError, models.APIError{
                        Code: 500, Message: "failed to resolve home directory",
                })
                return
        }

        if strings.HasPrefix(path, "~/") {
                path = filepath.Join(homeDir, path[2:])
        }

        fs := vfs.NewLocalVFS(claims.UID, claims.GID, homeDir)
        total, used, free, err := fs.DiskUsage(path)
        if err != nil {
                c.JSON(http.StatusInternalServerError, models.APIError{
                        Code: 500, Message: "failed to get disk usage", Details: err.Error(),
                })
                return
        }

        c.JSON(http.StatusOK, gin.H{
                "total": total,
                "used":  used,
                "free":  free,
        })
}
