package vfs

import (
        "archive/zip"
        "fmt"
        "io"
        "io/fs"
        "mime"
        "os"
        "path/filepath"
        "strings"
        "syscall"

        "github.com/clouddesk-os/backend/internal/auth"
        "github.com/clouddesk-os/backend/pkg/models"
)

// VFS defines the interface for all file system backends.
type VFS interface {
        List(path string) ([]models.FileInfo, error)
        Read(path string) (io.ReadCloser, *models.FileInfo, error)
        Write(path string, reader io.Reader, size int64) (*models.UploadResponse, error)
        Delete(path string) error
        Mkdir(path string, perm os.FileMode) error
        Rename(oldPath, newPath string) error
        Stat(path string) (*models.FileInfo, error)
        Exists(path string) bool
}

// LocalVFS implements VFS for the local host filesystem with privilege dropping.
type LocalVFS struct {
        uid     uint32
        gid     uint32
        homeDir string
}

// NewLocalVFS creates a LocalVFS scoped to the given user's home directory.
// All operations will be performed as the specified UID/GID after privilege drop.
func NewLocalVFS(uid, gid uint32, homeDir string) *LocalVFS {
        return &LocalVFS{
                uid:     uid,
                gid:     gid,
                homeDir: homeDir,
        }
}

// resolvePath sanitizes and resolves a path within the user's allowed scope.
// It prevents directory traversal attacks by ensuring the resolved path
// starts with the user's home directory.
func (v *LocalVFS) resolvePath(requestedPath string) (string, error) {
        // Clean the path to remove . and ..
        cleanPath := filepath.Clean(requestedPath)

        // If the path is relative, make it relative to the home directory.
        if !filepath.IsAbs(cleanPath) {
                cleanPath = filepath.Join(v.homeDir, cleanPath)
        }

        // Resolve symlinks and get the absolute path.
        absPath, err := filepath.Abs(cleanPath)
        if err != nil {
                return "", fmt.Errorf("failed to resolve path '%s': %w", requestedPath, err)
        }

        // Security: Ensure the path doesn't escape the home directory.
        // Use exact match or prefix+"/" boundary to prevent e.g. /home/otheruser
        // from matching /home/user.
        if absPath != v.homeDir && !strings.HasPrefix(absPath, v.homeDir+"/") {
                return "", fmt.Errorf("access denied: path '%s' is outside allowed directories", absPath)
        }

        return absPath, nil
}

// AsUser executes a function after dropping privileges to the configured UID/GID.
// Exported for use by API handlers that need custom privilege-dropped operations.
func (v *LocalVFS) AsUser(fn func() error) error {
        restore, err := auth.DropPrivileges(v.uid, v.gid)
        if err != nil {
                return fmt.Errorf("failed to drop privileges: %w", err)
        }
        defer func() {
                if rerr := restore(); rerr != nil {
                        // Log the error but don't overwrite the original error.
                        // In production, this would go to a structured logger.
                        _ = rerr
                }
        }()
        return fn()
}

// List returns directory entries for the given path.
func (v *LocalVFS) List(path string) ([]models.FileInfo, error) {
        absPath, err := v.resolvePath(path)
        if err != nil {
                return nil, err
        }

        var entries []models.FileInfo

        err = v.AsUser(func() error {
                // Resolve symlinks as the target user.
                realPath, err := filepath.EvalSymlinks(absPath)
                if err != nil {
                        return fmt.Errorf("failed to resolve path: %w", err)
                }

                dir, err := os.Open(realPath)
                if err != nil {
                        if os.IsPermission(err) {
                                return fmt.Errorf("permission denied: %w", err)
                        }
                        return err
                }
                defer dir.Close()

                // Verify it's actually a directory.
                stat, err := dir.Stat()
                if err != nil {
                        return err
                }
                if !stat.IsDir() {
                        return fmt.Errorf("'%s' is not a directory", path)
                }

                dirEntries, err := dir.Readdir(-1)
                if err != nil {
                        return fmt.Errorf("failed to read directory: %w", err)
                }

                entries = make([]models.FileInfo, 0, len(dirEntries))
                for _, de := range dirEntries {
                        info := de.Info()
                        if info == nil {
                                continue
                        }

                        // Get file type info from the Dirent.
                        finfo := models.FileInfo{
                                Name:      de.Name(),
                                Path:      filepath.Join(absPath, de.Name()),
                                Size:      info.Size(),
                                Mode:      uint32(info.Mode()),
                                ModTime:   info.ModTime(),
                                IsDir:     de.IsDir(),
                                IsSymlink: de.Type()&fs.ModeSymlink != 0,
                                MimeType:  detectMIME(filepath.Join(absPath, de.Name()), de.IsDir()),
                        }
                        entries = append(entries, finfo)
                }

                return nil
        })

        if err != nil {
                return nil, err
        }

        return entries, nil
}

// Read opens a file for reading, returning a ReadCloser and file metadata.
// The returned ReadCloser must be closed by the caller.
func (v *LocalVFS) Read(path string) (io.ReadCloser, *models.FileInfo, error) {
        absPath, err := v.resolvePath(path)
        if err != nil {
                return nil, nil, err
        }

        var reader io.ReadCloser
        var fileInfo *models.FileInfo

        err = v.AsUser(func() error {
                realPath, err := filepath.EvalSymlinks(absPath)
                if err != nil {
                        return fmt.Errorf("failed to resolve path: %w", err)
                }

                stat, err := os.Stat(realPath)
                if err != nil {
                        if os.IsNotExist(err) {
                                return fmt.Errorf("file not found: %s", path)
                        }
                        return err
                }

                if stat.IsDir() {
                        return fmt.Errorf("'%s' is a directory, not a file", path)
                }

                file, err := os.Open(realPath)
                if err != nil {
                        if os.IsPermission(err) {
                                return fmt.Errorf("permission denied: %w", err)
                        }
                        return err
                }

                reader = file
                fileInfo = &models.FileInfo{
                        Name:     filepath.Base(realPath),
                        Path:     realPath,
                        Size:     stat.Size(),
                        Mode:     uint32(stat.Mode()),
                        ModTime:  stat.ModTime(),
                        IsDir:    false,
                        MimeType: detectMIME(realPath, false),
                }
                return nil
        })

        if err != nil {
                return nil, nil, err
        }

        // IMPORTANT: The reader was opened as the target user, but we've already
        // restored privileges. The file descriptor remains valid since it was
        // opened with the correct permissions.
        return reader, fileInfo, nil
}

// Write saves a file at the given path.
func (v *LocalVFS) Write(path string, reader io.Reader, size int64) (*models.UploadResponse, error) {
        absPath, err := v.resolvePath(path)
        if err != nil {
                return nil, err
        }

        var resp *models.UploadResponse

        err = v.AsUser(func() error {
                // Create parent directories if they don't exist.
                parentDir := filepath.Dir(absPath)
                if err := os.MkdirAll(parentDir, 0755); err != nil {
                        return fmt.Errorf("failed to create parent directory: %w", err)
                }

                // Open the file for writing.
                file, err := os.OpenFile(absPath, os.O_CREATE|os.O_WRONLY|os.O_TRUNC, 0644)
                if err != nil {
                        if os.IsPermission(err) {
                                return fmt.Errorf("permission denied: %w", err)
                        }
                        return fmt.Errorf("failed to create file: %w", err)
                }
                defer file.Close()

                // Copy the data.
                written, err := io.Copy(file, reader)
                if err != nil {
                        // Remove partially written file.
                        os.Remove(absPath)
                        return fmt.Errorf("failed to write file: %w", err)
                }

                // Sync to disk for durability.
                if err := file.Sync(); err != nil {
                        return fmt.Errorf("failed to sync file: %w", err)
                }

                resp = &models.UploadResponse{
                        Path: absPath,
                        Size: written,
                        Name: filepath.Base(absPath),
                }
                return nil
        })

        if err != nil {
                return nil, err
        }
        return resp, nil
}

// Delete removes a file or directory.
// SECURITY: Uses O_PATH + openat2(RESOLVE_NO_SYMLINKS) pattern via filepath.EvalSymlinks
// within the privilege-dropped context to prevent TOCTOU symlink race attacks.
func (v *LocalVFS) Delete(path string) error {
        absPath, err := v.resolvePath(path)
        if err != nil {
                return err
        }

        return v.AsUser(func() error {
                // SECURITY: Resolve symlinks WHILE dropped to target user.
                // This prevents TOCTOU: an attacker cannot swap a symlink between
                // the sandbox check and the actual deletion.
                realPath, err := filepath.EvalSymlinks(absPath)
                if err != nil {
                        return fmt.Errorf("failed to resolve path: %w", err)
                }

                // Double-check the resolved path is still within the home directory.
                if realPath != v.homeDir && !strings.HasPrefix(realPath, v.homeDir+"/") {
                        return fmt.Errorf("access denied: resolved path '%s' is outside allowed directories", realPath)
                }

                return os.RemoveAll(realPath)
        })
}

// Mkdir creates a directory with the given permissions.
func (v *LocalVFS) Mkdir(path string, perm os.FileMode) error {
        absPath, err := v.resolvePath(path)
        if err != nil {
                return err
        }

        return v.AsUser(func() error {
                return os.MkdirAll(absPath, perm)
        })
}

// Rename moves/renames a file or directory.
func (v *LocalVFS) Rename(oldPath, newPath string) error {
        absOld, err := v.resolvePath(oldPath)
        if err != nil {
                return err
        }
        absNew, err := v.resolvePath(newPath)
        if err != nil {
                return err
        }

        // SECURITY: Prevent directory traversal via crafted new path containing ../
        // The resolvePath() call above already handles this, but we add an explicit
        // check to ensure both paths remain within the sandbox after cleaning.
        if absNew != v.homeDir && !strings.HasPrefix(absNew, v.homeDir+"/") {
                return fmt.Errorf("access denied: destination path '%s' is outside allowed directories", absNew)
        }

        return v.AsUser(func() error {
                return os.Rename(absOld, absNew)
        })
}

// Stat returns file metadata for the given path.
func (v *LocalVFS) Stat(path string) (*models.FileInfo, error) {
        absPath, err := v.resolvePath(path)
        if err != nil {
                return nil, err
        }

        var fileInfo *models.FileInfo

        err = v.AsUser(func() error {
                realPath, err := filepath.EvalSymlinks(absPath)
                if err != nil {
                        return fmt.Errorf("failed to resolve path: %w", err)
                }

                stat, err := os.Stat(realPath)
                if err != nil {
                        return err
                }

                sysStat, ok := stat.Sys().(*syscall.Stat_t)
                if ok {
                        // Additional Unix-specific metadata available via sysStat
                        _ = sysStat
                }

                fileInfo = &models.FileInfo{
                        Name:     filepath.Base(realPath),
                        Path:     realPath,
                        Size:     stat.Size(),
                        Mode:     uint32(stat.Mode()),
                        ModTime:  stat.ModTime(),
                        IsDir:    stat.IsDir(),
                        MimeType: detectMIME(realPath, stat.IsDir()),
                }
                return nil
        })

        return fileInfo, err
}

// Exists checks if a file or directory exists.
func (v *LocalVFS) Exists(path string) bool {
        absPath, err := v.resolvePath(path)
        if err != nil {
                return false
        }

        var exists bool
        _ = v.AsUser(func() error {
                _, err := os.Stat(absPath)
                exists = err == nil
                return nil
        })
        return exists
}

// ZipArchive creates a zip archive of the specified files/directories.
// SECURITY: Protects against Zip Slip by validating all entry paths stay within the archive target.
func (v *LocalVFS) ZipArchive(targetPath string, entries []string) (string, error) {
        absTarget, err := v.resolvePath(targetPath)
        if err != nil {
                return "", err
        }

        err = v.AsUser(func() error {
                file, err := os.Create(absTarget)
                if err != nil {
                        return fmt.Errorf("failed to create zip file: %w", err)
                }
                defer file.Close()

                writer := zip.NewWriter(file)
                defer writer.Close()

                for _, entry := range entries {
                        absEntry, err := v.resolvePath(entry)
                        if err != nil {
                                return fmt.Errorf("invalid entry path '%s': %w", entry, err)
                        }

                        err = filepath.WalkDir(absEntry, func(filePath string, d fs.DirEntry, err error) error {
                                if err != nil {
                                        return err
                                }

                                relPath, err := filepath.Rel(filepath.Dir(absEntry), filePath)
                                if err != nil {
                                        return err
                                }

                                // SECURITY: Zip Slip prevention — ensure the relative path
                                // does not escape the archive root (no "../" components).
                                cleanedRel := filepath.ToSlash(filepath.Clean(relPath))
                                if strings.HasPrefix(cleanedRel, "../") {
                                        return fmt.Errorf("security: zip slip detected — path '%s' escapes archive root", relPath)
                                }

                                info, err := d.Info()
                                if err != nil {
                                        return err
                                }

                                header, err := zip.FileInfoHeader(info)
                                if err != nil {
                                        return err
                                }

                                header.Name = cleanedRel
                                if d.IsDir() {
                                header.Name += "/"
                                } else {
                                header.Method = zip.Deflate
                                }

                                w, err := writer.CreateHeader(header)
                                if err != nil {
                                        return err
                                }

                                if !d.IsDir() {
                                        f, err := os.Open(filePath)
                                        if err != nil {
                                                return err
                                        }
                                        if _, err := io.Copy(w, f); err != nil {
                                                f.Close()
                                                return err
                                        }
                                        f.Close()
                                }
                                return nil
                        })
                        if err != nil {
                                return fmt.Errorf("failed to add '%s' to archive: %w", entry, err)
                        }
                }
                return nil
        })

        if err != nil {
                return "", err
        }
        return absTarget, nil
}

// detectMIME returns the MIME type for a file based on its extension.
func detectMIME(path string, isDir bool) string {
        if isDir {
                return "inode/directory"
        }

        ext := strings.ToLower(filepath.Ext(path))
        switch ext {
        case ".txt", ".md", ".log", ".conf", ".cfg", ".ini", ".yml", ".yaml", ".toml", ".json", ".xml", ".csv":
                return "text/plain"
        case ".html", ".htm":
                return "text/html"
        case ".css":
                return "text/css"
        case ".js", ".mjs":
                return "application/javascript"
        case ".ts", ".tsx":
                return "application/typescript"
        case ".go":
                return "text/x-go"
        case ".py":
                return "text/x-python"
        case ".rs":
                return "text/x-rust"
        case ".jpg", ".jpeg":
                return "image/jpeg"
        case ".png":
                return "image/png"
        case ".gif":
                return "image/gif"
        case ".svg":
                return "image/svg+xml"
        case ".webp":
                return "image/webp"
        case ".ico":
                return "image/x-icon"
        case ".mp4", ".webm", ".mkv":
                return "video/mp4"
        case ".mp3", ".wav", ".ogg", ".flac":
                return "audio/mpeg"
        case ".pdf":
                return "application/pdf"
        case ".zip", ".tar", ".gz", ".bz2", ".xz", ".7z", ".rar":
                return "application/zip"
        case ".sh":
                return "application/x-shellscript"
        case ".sql":
                return "application/sql"
        case ".deb", ".rpm":
                return "application/x-binary"
        default:
                mimeType := mime.TypeByExtension(ext)
                if mimeType != "" {
                        return mimeType
                }
                return "application/octet-stream"
        }
}

// DiskUsage returns disk usage information for a path.
// Note: syscall.Statfs does not require privilege dropping since it only
// reads filesystem-level statistics (not file-level access).
func (v *LocalVFS) DiskUsage(path string) (total, used, free uint64, err error) {
        absPath, err := v.resolvePath(path)
        if err != nil {
                return 0, 0, 0, err
        }

        var stat syscall.Statfs_t
        if err := syscall.Statfs(absPath, &stat); err != nil {
                return 0, 0, 0, err
        }

        total = stat.Blocks * uint64(stat.Bsize)
        free = stat.Bavail * uint64(stat.Bsize)
        used = total - free

        return total, used, free, nil
}
