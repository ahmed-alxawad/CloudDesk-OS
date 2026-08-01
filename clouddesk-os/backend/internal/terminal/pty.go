package terminal

import (
	"encoding/json"
	"fmt"
	"log"
	"os"
	"os/exec"
	"sync"
	"syscall"
	"unsafe"

	"github.com/clouddesk-os/backend/internal/auth"
	"github.com/gorilla/websocket"
)

// Session represents an active PTY session for a single user.
// It manages the lifecycle of the forked shell process and the PTY file descriptor.
// SECURITY: The shell process runs as the authenticated Linux user (not root).
// This is enforced via SysProcAttr.Credential, which sets UID/GID at fork time.
type Session struct {
	Username string
	UID      uint32
	GID      uint32
	HomeDir  string
	Shell    string
	ptyFD    *os.File
	cmd      *exec.Cmd
	wsConn   *websocket.Conn
	mu       sync.Mutex
	closed   bool
}

// ResizeMessage is sent from the frontend to resize the PTY.
type ResizeMessage struct {
	Cols uint16 `json:"cols"`
	Rows uint16 `json:"rows"`
}

// NewSession creates a new PTY session for the given user.
// The shell process is spawned with the user's UID/GID and their login shell.
func NewSession(username string, uid, gid uint32, homeDir, shell string) (*Session, error) {
	if shell == "" || shell == "/bin/false" || shell == "/usr/sbin/nologin" {
		return nil, fmt.Errorf("user '%s' has no valid login shell (%s)", username, shell)
	}

	// Verify the shell binary exists.
	if _, err := os.Stat(shell); err != nil {
		return nil, fmt.Errorf("user shell '%s' not found: %w", shell, err)
	}

	return &Session{
		Username: username,
		UID:      uid,
		GID:      gid,
		HomeDir:  homeDir,
		Shell:    shell,
	}, nil
}

// Start forks the shell process with a new PTY.
// SECURITY: The process runs as the authenticated user, NOT as root.
// We use SysProcAttr.Credential (fork-time UID/GID) rather than
// Setresuid (runtime privilege drop) because PTY file descriptors
// created before privilege drop would retain root ownership,
// creating a confused deputy vulnerability.
func (s *Session) Start(cols, rows uint16) error {
	// Create PTY master/slave pair.
	ptyMaster, ptySlave, err := ptyOpen()
	if err != nil {
		return fmt.Errorf("failed to create PTY: %w", err)
	}

	// Set initial terminal size.
	if err := ptySetSize(ptyMaster, cols, rows); err != nil {
		ptyMaster.Close()
		ptySlave.Close()
		return fmt.Errorf("failed to set PTY size: %w", err)
	}

	// Start the user's shell attached to the PTY slave.
	// SECURITY: SysProcAttr.Credential sets UID/GID at fork() time,
	// so the child process NEVER runs as root. This is the safest
	// approach — no runtime privilege escalation is possible.
	cmd := exec.Command(s.Shell, "--login")
	cmd.SysProcAttr = &syscall.SysProcAttr{
		Credential: &syscall.Credential{
			Uid:    s.UID,
			Gid:    s.GID,
			Groups: []uint32{s.GID},
		},
		Setctty: true,
		Setsid:  true,
	}
	cmd.Dir = s.HomeDir
	cmd.Env = append(
		os.Environ(),
		fmt.Sprintf("HOME=%s", s.HomeDir),
		fmt.Sprintf("USER=%s", s.Username),
		fmt.Sprintf("LOGNAME=%s", s.Username),
		fmt.Sprintf("SHELL=%s", s.Shell),
		"TERM=xterm-256color",
		"LANG=en_US.UTF-8",
	)
	cmd.Stdin = ptySlave
	cmd.Stdout = ptySlave
	cmd.Stderr = ptySlave

	if err := cmd.Start(); err != nil {
		ptyMaster.Close()
		ptySlave.Close()
		return fmt.Errorf("failed to start shell: %w", err)
	}

	// Close the slave FD in the parent process — only the child needs it.
	ptySlave.Close()

	s.ptyFD = ptyMaster
	s.cmd = cmd

	// Monitor the shell process in a goroutine.
	go func() {
		_ = cmd.Wait()
		s.mu.Lock()
		defer s.mu.Unlock()
		if s.ptyFD != nil {
			s.ptyFD.Close()
			s.ptyFD = nil
		}
		s.closed = true
	}()

	return nil
}

// Attach connects a WebSocket connection to this session.
// It starts bidirectional I/O between the WebSocket and the PTY.
func (s *Session) Attach(wsConn *websocket.Conn, cols, rows uint16) {
	s.mu.Lock()
	if s.wsConn != nil {
		s.mu.Unlock()
		return // Already attached
	}
	s.wsConn = wsConn
	ptyFD := s.ptyFD
	s.mu.Unlock()

	if ptyFD == nil {
		wsConn.WriteMessage(websocket.TextMessage, []byte("\r\n\033[31mSession has ended. Please open a new terminal.\033[0m\r\n"))
		wsConn.Close()
		return
	}

	// Set terminal size.
	_ = ptySetSize(ptyFD, cols, rows)

	// Goroutine 1: PTY → WebSocket (terminal output to browser).
	go s.copyToWebSocket(ptyFD, wsConn)

	// Goroutine 2: WebSocket → PTY (user input to terminal).
	// This blocks until the WebSocket connection is closed.
	s.copyFromWebSocket(ptyFD, wsConn)
}

// copyToWebSocket reads from the PTY master and writes to the WebSocket.
func (s *Session) copyToWebSocket(pty *os.File, ws *websocket.Conn) {
	buf := make([]byte, 8192)
	for {
		n, err := pty.Read(buf)
		if n > 0 {
			s.mu.Lock()
			if s.closed {
				s.mu.Unlock()
				return
			}
			err := ws.WriteMessage(websocket.TextMessage, buf[:n])
			s.mu.Unlock()
			if err != nil {
				return
			}
		}
		if err != nil {
			s.mu.Lock()
			if !s.closed {
				_ = ws.WriteMessage(websocket.TextMessage, []byte("\r\n\033[90m[Session ended]\033[0m\r\n"))
				s.closed = true
			}
			s.mu.Unlock()
			return
		}
	}
}

// copyFromWebSocket reads messages from the WebSocket and writes to the PTY.
func (s *Session) copyFromWebSocket(pty *os.File, ws *websocket.Conn) {
	for {
		_, msg, err := ws.ReadMessage()
		if err != nil {
			return
		}

		// Raw text input (most common) — just write bytes to PTY.
		if len(msg) > 0 && msg[0] != '{' {
			s.mu.Lock()
			if s.ptyFD != nil {
				_, _ = s.ptyFD.Write(msg)
			}
			s.mu.Unlock()
			continue
		}

		// Parse JSON control messages (resize).
		var ctrl struct {
			Type string `json:"type"`
		}
		if json.Unmarshal(msg, &ctrl) == nil {
			switch ctrl.Type {
			case "resize":
				var resize ResizeMessage
				if json.Unmarshal(msg, &resize) == nil {
					s.mu.Lock()
					if s.ptyFD != nil {
						_ = ptySetSize(s.ptyFD, resize.Cols, resize.Rows)
					}
					s.mu.Unlock()
			}
			}
		}
	}
}

// Close terminates the session.
func (s *Session) Close() {
	s.mu.Lock()
	defer s.mu.Unlock()

	if s.closed {
		return
	}
	s.closed = true

	if s.wsConn != nil {
		_ = s.wsConn.Close()
		s.wsConn = nil
	}

	if s.ptyFD != nil {
		_ = s.ptyFD.Close()
		s.ptyFD = nil
	}

	if s.cmd != nil && s.cmd.Process != nil {
		_ = s.cmd.Process.Signal(syscall.SIGTERM)
	}
}

// ResolveShell gets the user's login shell from /etc/passwd.
func ResolveShell(username string) string {
	info, err := auth.ResolveUser(username)
	if err != nil {
		log.Printf("[WARN] Failed to resolve shell for user '%s': %v", username, err)
		return "/bin/bash"
	}
	return info.Shell
}

// ──────────────────────────────────────────────────────────────────────
// PTY system calls (Linux-specific)
// ──────────────────────────────────────────────────────────────────────

// ptyOpen creates a new PTY master/slave pair using ioctl.
func ptyOpen() (master *os.File, slave *os.File, err error) {
	masterFD, err := syscall.Open("/dev/ptmx", syscall.O_RDWR|syscall.O_NOCTTY|syscall.O_CLOEXEC, 0)
	if err != nil {
		return nil, nil, fmt.Errorf("failed to open /dev/ptmx: %w", err)
	}

	master = os.NewFile(uintptr(masterFD), "/dev/ptmx")
	if master == nil {
		syscall.Close(masterFD)
		return nil, nil, fmt.Errorf("failed to create master file")
	}

	// Unlock the slave PTY.
	var num int
	_, _, errno := syscall.Syscall(
		syscall.SYS_IOCTL,
		uintptr(masterFD),
		uintptr(syscall.TIOCSPTLCK),
		uintptr(unsafe.Pointer(&num)),
	)
	if errno != 0 {
		master.Close()
		return nil, nil, fmt.Errorf("TIOCSPTLCK failed: %v", errno)
	}

	// Get the slave PTY number.
	var slaveNo int
	_, _, errno = syscall.Syscall(
		syscall.SYS_IOCTL,
		uintptr(masterFD),
		uintptr(syscall.TIOCGPTN),
		uintptr(unsafe.Pointer(&slaveNo)),
	)
	if errno != 0 {
		master.Close()
		return nil, nil, fmt.Errorf("TIOCGPTN failed: %v", errno)
	}

	slavePath := fmt.Sprintf("/dev/pts/%d", slaveNo)
	slaveFD, err := syscall.Open(slavePath, syscall.O_RDWR|syscall.O_NOCTTY, 0)
	if err != nil {
		master.Close()
		return nil, nil, fmt.Errorf("failed to open slave PTY %s: %w", slavePath, err)
	}

	slave = os.NewFile(uintptr(slaveFD), slavePath)
	if slave == nil {
		syscall.Close(slaveFD)
		master.Close()
		return nil, nil, fmt.Errorf("failed to create slave file")
	}

	return master, slave, nil
}

// winsize mirrors the C struct winsize for ioctl TIOCSWINSZ.
type winsize struct {
	WSRow    uint16
	WSCol    uint16
	WSXPixel uint16
	WSYPixel uint16
}

// ptySetSize sets the terminal window size via ioctl TIOCSWINSZ.
func ptySetSize(pty *os.File, cols, rows uint16) error {
	ws := &winsize{
		WSRow: rows,
		WSCol: cols,
	}
	_, _, errno := syscall.Syscall(
		syscall.SYS_IOCTL,
		pty.Fd(),
		uintptr(syscall.TIOCSWINSZ),
		uintptr(unsafe.Pointer(ws)),
	)
	if errno != 0 {
		return fmt.Errorf("TIOCSWINSZ failed: %v", errno)
	}
	return nil
}
