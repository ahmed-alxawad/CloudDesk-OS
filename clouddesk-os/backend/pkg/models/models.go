package models

import (
	"database/sql"
	"time"
)

// User represents a local OS user stored in our metadata DB.
type User struct {
	ID          int64          `json:"id" db:"id"`
	Username    string         `json:"username" db:"username"`
	UID         uint32         `json:"uid" db:"uid"`
	GID         uint32         `json:"gid" db:"gid"`
	HomeDir     string         `json:"home_dir" db:"home_dir"`
	Shell       string         `json:"shell" db:"shell"`
	Role        string         `json:"role" db:"role"` // admin, user, viewer
	IsActive    bool           `json:"is_active" db:"is_active"`
	TwoFactorSecret sql.NullString `json:"two_factor_secret,omitempty" db:"two_factor_secret"`
	CreatedAt   time.Time      `json:"created_at" db:"created_at"`
	LastLogin   sql.NullTime   `json:"last_login,omitempty" db:"last_login"`
}

// SSHKey stores an encrypted SSH private key.
type SSHKey struct {
	ID                 int64     `json:"id" db:"id"`
	UserID             int64     `json:"user_id" db:"user_id"`
	Label              string    `json:"label" db:"label"`
	EncryptedPrivateKey string    `json:"-" db:"encrypted_private_key"` // Never expose to frontend
	PublicKey          string    `json:"public_key" db:"public_key"`
	Host               string    `json:"host" db:"host"`
	Port               int       `json:"port" db:"port"`
	Username           string    `json:"username" db:"username"`
	Fingerprint        string    `json:"fingerprint" db:"fingerprint"`
	CreatedAt          time.Time `json:"created_at" db:"created_at"`
	UpdatedAt          time.Time `json:"updated_at" db:"updated_at"`
}

// RemoteConnection represents a saved remote server connection.
type RemoteConnection struct {
	ID           int64     `json:"id" db:"id"`
	UserID       int64     `json:"user_id" db:"user_id"`
	Label        string    `json:"label" db:"label"`
	Protocol     string    `json:"protocol" db:"protocol"` // sftp, scp, smb, s3, webdav
	Host         string    `json:"host" db:"host"`
	Port         int       `json:"port" db:"port"`
	SSHKeyID     sql.NullInt64 `json:"ssh_key_id,omitempty" db:"ssh_key_id"`
	Username     string    `json:"username" db:"username"`
	CredentialsRef string  `json:"-" db:"credentials_ref"`
	MountPath    string    `json:"mount_path" db:"mount_path"`
	IsConnected  bool      `json:"is_connected" db:"is_connected"`
	CreatedAt    time.Time `json:"created_at" db:"created_at"`
	UpdatedAt    time.Time `json:"updated_at" db:"updated_at"`
}

// TransferJob represents a file transfer operation.
type TransferJob struct {
	ID          int64          `json:"id" db:"id"`
	UserID      int64          `json:"user_id" db:"user_id"`
	SourcePath  string         `json:"source_path" db:"source_path"`
	DestPath    string         `json:"dest_path" db:"dest_path"`
	SourceProto string         `json:"source_protocol" db:"source_protocol"`
	DestProto   string         `json:"dest_protocol" db:"dest_protocol"`
	Status      string         `json:"status" db:"status"` // queued, running, completed, failed, cancelled
	Progress    float64        `json:"progress" db:"progress"`
	TotalBytes  int64          `json:"total_bytes" db:"total_bytes"`
	Transferred int64          `json:"transferred" db:"transferred"`
	SpeedBps    int64          `json:"speed_bps" db:"speed_bps"`
	ErrorLog    sql.NullString `json:"error_log,omitempty" db:"error_log"`
	CreatedAt   time.Time      `json:"created_at" db:"created_at"`
	StartedAt   sql.NullTime   `json:"started_at,omitempty" db:"started_at"`
	CompletedAt sql.NullTime   `json:"completed_at,omitempty" db:"completed_at"`
}

// AuditLog records user actions for compliance and security.
type AuditLog struct {
	ID        int64     `json:"id" db:"id"`
	UserID    int64     `json:"user_id" db:"user_id"`
	Username  string    `json:"username" db:"username"`
	Action    string    `json:"action" db:"action"`
	FilePath  string    `json:"file_path,omitempty" db:"file_path"`
	IPAddress string    `json:"ip_address" db:"ip_address"`
	UserAgent string    `json:"user_agent,omitempty" db:"user_agent"`
	Details   string    `json:"details,omitempty" db:"details"`
	Timestamp time.Time `json:"timestamp" db:"timestamp"`
}

// LoginRequest is the JSON body for login attempts.
type LoginRequest struct {
	Username string `json:"username" binding:"required,min=1,max=64"`
	Password string `json:"password" binding:"required,min=1"`
}

// LoginResponse is returned after successful authentication.
type LoginResponse struct {
	Token     string `json:"token"`
	User      User   `json:"user"`
	ExpiresAt int64  `json:"expires_at"`
}

// FileInfo is returned by VFS listing operations.
type FileInfo struct {
	Name      string      `json:"name"`
	Path      string      `json:"path"`
	Size      int64       `json:"size"`
	Mode      uint32      `json:"mode"`
	ModTime   time.Time   `json:"mod_time"`
	IsDir     bool        `json:"is_dir"`
	IsSymlink bool        `json:"is_symlink"`
	MimeType  string      `json:"mime_type,omitempty"`
}

// UploadResponse is returned after successful file upload.
type UploadResponse struct {
	Path string `json:"path"`
	Size int64  `json:"size"`
	Name string `json:"name"`
}

// APIError is a standard error response.
type APIError struct {
	Code    int    `json:"code"`
	Message string `json:"message"`
	Details string `json:"details,omitempty"`
}
