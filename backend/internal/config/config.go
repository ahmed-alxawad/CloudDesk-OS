package config

import (
        "crypto/rand"
        "encoding/base64"
        "flag"
        "fmt"
        "os"
        "strconv"
        "strings"
        "sync"
)

var (
        once     sync.Once
        instance *Config

        // These are set by flag.Parse() and used after Load().
        jwtSecretFile string
        masterKeyFile string
)

func Load() *Config {
        once.Do(func() {
                instance = &Config{}

                flag.StringVar(&instance.Server.Host, "host", "127.0.0.1", "Server bind host")
                flag.IntVar(&instance.Server.Port, "port", 8080, "Server bind port")

                flag.StringVar(&instance.Database.Host, "db-host", "localhost", "PostgreSQL host")
                flag.IntVar(&instance.Database.Port, "db-port", 5432, "PostgreSQL port")
                flag.StringVar(&instance.Database.User, "db-user", "clouddesk", "PostgreSQL user")
                flag.StringVar(&instance.Database.Password, "db-password", "", "PostgreSQL password")
                flag.StringVar(&instance.Database.Name, "db-name", "clouddesk", "PostgreSQL database name")
                flag.StringVar(&instance.Database.SSLMode, "db-sslmode", "disable", "PostgreSQL SSL mode")

                flag.StringVar(&instance.JWT.Secret, "jwt-secret", "", "JWT signing secret (or use -jwt-secret-file)")
                flag.IntVar(&instance.JWT.ExpirationHours, "jwt-expiry", 24, "JWT token expiration in hours")

                flag.StringVar(&instance.Security.MasterEncryptionKey, "master-key", "", "AES-256 master key (or use -master-key-file)")
                flag.StringVar(&instance.Security.AllowedOrigins, "allowed-origins", "*", "CORS allowed origins")

                flag.StringVar(&instance.CodeServer.BinPath, "code-server-bin", "/usr/bin/code-server", "Path to code-server binary")
                flag.StringVar(&instance.CodeServer.DataDir, "code-server-data", "/var/lib/clouddesk/code-server", "code-server data directory")
                flag.StringVar(&instance.CodeServer.SocketDir, "code-server-sock", "/var/run/clouddesk", "code-server socket directory")
                flag.IntVar(&instance.CodeServer.DefaultPort, "code-server-port", 0, "Fallback TCP port for code-server (0 = socket only)")
                flag.StringVar(&instance.CodeServer.ExtraArgs, "code-server-args", "", "Extra args for code-server")

                flag.StringVar(&instance.VFS.HomeBasePath, "home-base", "/home", "Base path for home directories")

                flag.BoolVar(&instance.Audit.Enabled, "audit", true, "Enable audit logging")

                // File-based secret flags (alternative to inline -jwt-secret / -master-key)
                flag.StringVar(&jwtSecretFile, "jwt-secret-file", "", "Path to file containing JWT secret")
                flag.StringVar(&masterKeyFile, "master-key-file", "", "Path to file containing AES-256 master key")

                flag.Parse()

                // Load secrets from files if specified (overrides -jwt-secret / env var)
                if jwtSecretFile != "" {
                        if secret, err := ReadSecretFromFile(jwtSecretFile); err != nil {
                                fmt.Fprintf(os.Stderr, "FATAL: failed to read JWT secret file: %v\n", err)
                                os.Exit(1)
                        } else {
                                instance.JWT.Secret = secret
                        }
                }
                if masterKeyFile != "" {
                        if key, err := ReadSecretFromFile(masterKeyFile); err != nil {
                                fmt.Fprintf(os.Stderr, "FATAL: failed to read master key file: %v\n", err)
                                os.Exit(1)
                        } else {
                                instance.Security.MasterEncryptionKey = key
                        }
                }

                // Environment variable overrides (only apply if no flag-based secret was set).
                // SECURITY: Env vars must NOT override file-based secrets, otherwise an attacker
                // who can set env vars (e.g., via a web shell) could weaken the installed configuration.
                if v := os.Getenv("CLOUDDESK_DB_HOST"); v != "" {
                        instance.Database.Host = v
                }
                if v := os.Getenv("CLOUDDESK_DB_PASSWORD"); v != "" {
                        instance.Database.Password = v
                }
                if jwtSecretFile == "" && instance.JWT.Secret == "" {
                        if v := os.Getenv("CLOUDDESK_JWT_SECRET"); v != "" {
                                instance.JWT.Secret = v
                        }
                }
                if masterKeyFile == "" && instance.Security.MasterEncryptionKey == "" {
                        if v := os.Getenv("CLOUDDESK_MASTER_KEY"); v != "" {
                                instance.Security.MasterEncryptionKey = v
                        }
                }
                if v := os.Getenv("CLOUDDESK_PORT"); v != "" {
                        if port, err := strconv.Atoi(v); err == nil {
                                instance.Server.Port = port
                        }
                }

                instance.validate()
        })
        return instance
}

func (c *Config) validate() {
        if c.JWT.Secret == "" {
                fmt.Fprintln(os.Stderr, "WARNING: JWT secret not set. Generating random secret (sessions will not survive restarts)")
                c.JWT.Secret = generateRandomSecret(32)
        }
        if c.Security.MasterEncryptionKey == "" {
                fmt.Fprintln(os.Stderr, "WARNING: Master encryption key not set. SSH key encryption will use a derived key.")
                c.Security.MasterEncryptionKey = generateRandomSecret(32)
        }
}

func (c *Config) DSN() string {
        return fmt.Sprintf(
                "host=%s port=%d user=%s password=%s dbname=%s sslmode=%s",
                c.Database.Host, c.Database.Port, c.Database.User,
                c.Database.Password, c.Database.Name, c.Database.SSLMode,
        )
}

// Version returns the application version string.
// Set via -ldflags: -ldflags="-X github.com/clouddesk-os/backend/internal/config.version=x.y.z"
func (c *Config) Version() string {
        return version
}

// version is set via -ldflags at build time.
var version = "0.2.0"

func generateRandomSecret(length int) string {
        b := make([]byte, length)
        if _, err := rand.Read(b); err != nil {
                panic(fmt.Sprintf("failed to generate random secret: %v", err))
        }
        return base64.StdEncoding.EncodeToString(b)
}

// ReadSecretFromFile reads a file and returns its contents as a trimmed string.
func ReadSecretFromFile(path string) (string, error) {
        data, err := os.ReadFile(path)
        if err != nil {
                return "", fmt.Errorf("failed to read secret file %s: %w", path, err)
        }
        return strings.TrimSpace(string(data)), nil
}
