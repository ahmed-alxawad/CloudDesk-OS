package ide

import (
        "context"
        "fmt"
        "net"
        "os"
        "os/exec"
        "path/filepath"
        "strings"
        "sync"
        "syscall"
        "time"

        "github.com/clouddesk-os/backend/internal/auth"
)

// Status represents the state of a code-server instance.
type Status string

const (
        StatusStopped  Status = "stopped"
        StatusStarting Status = "starting"
        StatusRunning  Status = "running"
        StatusError    Status = "error"
)

// Instance tracks a single user's code-server process.
type Instance struct {
        UserID   uint32
        GID      uint32
        Username string
        HomeDir  string
        Socket   string
        PID      int
        Status   Status
        Cmd      *exec.Cmd
        mu       sync.Mutex
        cancel   context.CancelFunc
}

// Manager manages all code-server instances.
type Manager struct {
        instances map[string]*Instance // keyed by username
        socketDir string
        dataDir   string
        binPath   string
        extraArgs string
        mu        sync.RWMutex
}

// NewManager creates a new code-server Manager.
func NewManager(binPath, socketDir, dataDir, extraArgs string) (*Manager, error) {
        // Ensure directories exist.
        if err := os.MkdirAll(socketDir, 0755); err != nil {
                return nil, fmt.Errorf("failed to create socket directory '%s': %w", socketDir, err)
        }
        if err := os.MkdirAll(dataDir, 0755); err != nil {
                return nil, fmt.Errorf("failed to create data directory '%s': %w", dataDir, err)
        }

        m := &Manager{
                instances: make(map[string]*Instance),
                socketDir: socketDir,
                dataDir:   dataDir,
                binPath:   binPath,
                extraArgs: extraArgs,
        }

        // Discover any running instances from previous sessions.
        m.discoverExisting()

        return m, nil
}

// GetInstance returns the instance for a user, creating one if needed.
// It does NOT block — it returns immediately with the current state.
func (m *Manager) GetInstance(username string, uid uint32, gid uint32, homeDir string) (*Instance, error) {
        m.mu.RLock()
        inst, exists := m.instances[username]
        m.mu.RUnlock()

        if exists {
                return inst, nil
        }

        // Create a new instance.
        inst = &Instance{
                UserID:   uid,
                GID:      gid,
                Username: username,
                HomeDir:  homeDir,
                Socket:   filepath.Join(m.socketDir, username+".sock"),
                Status:   StatusStopped,
        }

        m.mu.Lock()
        // Double-check after acquiring write lock.
        if existing, ok := m.instances[username]; ok {
                m.mu.Unlock()
                return existing, nil
        }
        m.instances[username] = inst
        m.mu.Unlock()

        return inst, nil
}

// Start launches code-server for the given user if not already running.
func (m *Manager) Start(inst *Instance) error {
        inst.mu.Lock()
        defer inst.mu.Unlock()

        if inst.Status == StatusRunning {
                // Check if the process is actually alive.
                if inst.Cmd != nil && inst.Cmd.Process != nil {
                        if err := inst.Cmd.Process.Signal(syscall.Signal(0)); err == nil {
                                return nil // Already running.
                        }
                }
                inst.Status = StatusStopped
        }

        inst.Status = StatusStarting

        // Ensure the socket directory has correct permissions.
        if err := os.Chmod(m.socketDir, 0755); err != nil {
                inst.Status = StatusError
                return fmt.Errorf("failed to set socket directory permissions: %w", err)
        }

        // Ensure user's data directory exists with correct ownership.
        userDataDir := filepath.Join(m.dataDir, inst.Username)
        if err := os.MkdirAll(userDataDir, 0755); err != nil {
                inst.Status = StatusError
                return fmt.Errorf("failed to create user data directory: %w", err)
        }
        // Chown the user's data directory so they can write to it.
        if err := os.Chown(userDataDir, int(inst.UserID), int(inst.GID)); err != nil {
                inst.Status = StatusError
                return fmt.Errorf("failed to chown user data directory: %w", err)
        }

        // Build code-server arguments.
        args := []string{
                "--socket", inst.Socket,
                "--user-data-dir", userDataDir,
                "--extensions-dir", filepath.Join(userDataDir, "extensions"),
                "--home", inst.HomeDir,
                "--auth", "none", // Auth is handled by CloudDesk OS PAM
                "--disable-telemetry",
                "--disable-update-check",
                "--no-sandbox", // Required when running without display
        }

        if m.extraArgs != "" {
                args = append(args, strings.Fields(m.extraArgs)...)
        }

        ctx, cancel := context.WithCancel(context.Background())
        inst.cancel = cancel

        // Open /dev/null for stdin.
        devNull, err := os.Open("/dev/null")
        if err != nil {
                cancel()
                inst.Status = StatusError
                return fmt.Errorf("failed to open /dev/null: %w", err)
        }
        defer devNull.Close()

        cmd := exec.CommandContext(ctx, m.binPath, args...)
        cmd.Stdout = devNull
        cmd.Stderr = devNull
        cmd.Stdin = devNull
        cmd.Dir = inst.HomeDir

        // Use SysProcAttr.Credential to run code-server as the target user.
        // This is the correct way to fork a child process as a different user —
        // it does NOT affect the parent process's UID/GID.
        cmd.SysProcAttr = &syscall.SysProcAttr{
                Credential: &syscall.Credential{
                        Uid: inst.UserID,
                        Gid: inst.GID,
                },
                Setsid: true, // Create new session so signals don't propagate from parent.
        }

        err = cmd.Start()
        if err != nil {
                cancel()
                inst.Status = StatusError
                return fmt.Errorf("failed to start code-server: %w", err)
        }

        inst.Cmd = cmd
        inst.PID = cmd.Process.Pid

        // Wait for the socket to appear (with timeout).
        socketReady := make(chan error, 1)
        go func() {
                for i := 0; i < 60; i++ {
                        time.Sleep(500 * time.Millisecond)
                        if _, err := net.Dial("unix", inst.Socket); err == nil {
                                socketReady <- nil
                                return
                        }
                }
                socketReady <- fmt.Errorf("code-server socket did not appear within 30 seconds")
        }()

        select {
        case err := <-socketReady:
                if err != nil {
                        inst.Status = StatusError
                        return err
                }
        case <-time.After(35 * time.Second):
                inst.Status = StatusError
                return fmt.Errorf("timed out waiting for code-server socket")
        }

        // Set socket permissions to allow the proxy (running as root) to connect.
        if err := os.Chmod(inst.Socket, 0660); err != nil {
                inst.Status = StatusError
                return fmt.Errorf("failed to set socket permissions: %w", err)
        }

        inst.Status = StatusRunning

        // Monitor the process in a goroutine.
        go m.monitorProcess(inst)

        return nil
}

// Stop terminates the code-server process for a user.
func (m *Manager) Stop(inst *Instance) error {
        inst.mu.Lock()
        defer inst.mu.Unlock()

        if inst.Status != StatusRunning && inst.Status != StatusStarting {
                return nil
        }

        if inst.cancel != nil {
                inst.cancel()
        }

        if inst.Cmd != nil && inst.Cmd.Process != nil {
                // Graceful shutdown with timeout.
                done := make(chan error, 1)
                go func() {
                        _ = inst.Cmd.Wait()
                        done <- nil
                }()

                // Send SIGTERM.
                if err := inst.Cmd.Process.Signal(syscall.SIGTERM); err != nil {
                        // Process may have already exited.
                }

                select {
                case <-done:
                case <-time.After(10 * time.Second):
                        // Force kill.
                        _ = inst.Cmd.Process.Kill()
                        <-done
                }
        }

        // Clean up socket.
        os.Remove(inst.Socket)

        inst.Status = StatusStopped
        inst.PID = 0
        inst.Cmd = nil

        return nil
}

// GetStatus returns the current status of a user's code-server.
func (m *Manager) GetStatus(username string) Status {
        m.mu.RLock()
        inst, exists := m.instances[username]
        m.mu.RUnlock()

        if !exists {
                return StatusStopped
        }

        inst.mu.Lock()
        defer inst.mu.Unlock()
        return inst.Status
}

// ProxyConn connects to the code-server Unix socket and returns the connection.
// The caller is responsible for closing the connection.
func (m *Manager) ProxyConn(username string) (net.Conn, error) {
        inst, err := m.GetInstance(username, 0, 0, "")
        if err != nil {
                return nil, err
        }

        inst.mu.Lock()
        status := inst.Status
        socket := inst.Socket
        inst.mu.Unlock()

        if status != StatusRunning {
                return nil, fmt.Errorf("code-server is not running for user '%s' (status: %s)", username, status)
        }

        conn, err := net.Dial("unix", socket)
        if err != nil {
                return nil, fmt.Errorf("failed to connect to code-server socket: %w", err)
        }

        return conn, nil
}

// ListInstances returns all managed instances.
func (m *Manager) ListInstances() map[string]Status {
        m.mu.RLock()
        defer m.mu.RUnlock()

        result := make(map[string]Status, len(m.instances))
        for username, inst := range m.instances {
                inst.mu.Lock()
                result[username] = inst.Status
                inst.mu.Unlock()
        }
        return result
}

// monitorProcess watches a code-server process and updates its status.
func (m *Manager) monitorProcess(inst *Instance) {
        if inst.Cmd == nil {
                return
        }

        err := inst.Cmd.Wait()

        inst.mu.Lock()
        defer inst.mu.Unlock()

        if inst.Status == StatusRunning || inst.Status == StatusStarting {
                if err != nil {
                        inst.Status = StatusError
                } else {
                        inst.Status = StatusStopped
                }
                inst.PID = 0
        }
}

// discoverExisting finds code-server sockets from previous sessions.
func (m *Manager) discoverExisting() {
        entries, err := os.ReadDir(m.socketDir)
        if err != nil {
                return
        }

        for _, entry := range entries {
                if entry.IsDir() {
                        continue
                }

                name := entry.Name()
                if !strings.HasSuffix(name, ".sock") {
                        continue
                }

                username := strings.TrimSuffix(name, ".sock")
                if username == "" {
                        continue
                }

                // Try to connect to verify it's alive.
                socketPath := filepath.Join(m.socketDir, name)
                conn, err := net.Dial("unix", socketPath)
                if err != nil {
                        // Stale socket, clean it up.
                        os.Remove(socketPath)
                        continue
                }
                conn.Close()

                // Socket is alive — reconstruct the instance.
                userInfo, err := auth.ResolveUser(username)
                if err != nil {
                        continue
                }

                inst := &Instance{
                        UserID:   userInfo.UID,
                        GID:      userInfo.GID,
                        Username: username,
                        HomeDir:  userInfo.HomeDir,
                        Socket:   socketPath,
                        Status:   StatusRunning,
                }

                m.mu.Lock()
                m.instances[username] = inst
                m.mu.Unlock()
        }
}
