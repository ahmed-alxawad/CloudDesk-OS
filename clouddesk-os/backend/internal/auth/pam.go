//go:build linux && cgo

package auth

/*
#cgo LDFLAGS: -lpam
#include <security/pam_appl.h>
#include <stdlib.h>
#include <string.h>

// PAM conversation callback that supplies the password.
static int pam_conv_cb(int num_msg, const struct pam_message **msgm,
                       struct pam_response **response, void *appdata_ptr) {
    if (num_msg <= 0 || num_msg > PAM_MAX_NUM_MSG) {
        return PAM_CONV_ERR;
    }

    *response = (struct pam_response *)calloc(num_msg, sizeof(struct pam_response));
    if (*response == NULL) {
        return PAM_BUF_ERR;
    }

    const char *password = (const char *)appdata_ptr;

    for (int i = 0; i < num_msg; i++) {
        switch (msgm[i]->msg_style) {
            case PAM_PROMPT_ECHO_OFF:
                (*response)[i].resp = strdup(password);
                if ((*response)[i].resp == NULL) {
                    for (int j = 0; j < i; j++) {
                        free((*response)[j].resp);
                    }
                    free(*response);
                    *response = NULL;
                    return PAM_BUF_ERR;
                }
                break;
            case PAM_PROMPT_ECHO_ON:
                (*response)[i].resp = strdup(password);
                if ((*response)[i].resp == NULL) {
                    for (int j = 0; j < i; j++) {
                        free((*response)[j].resp);
                    }
                    free(*response);
                    *response = NULL;
                    return PAM_BUF_ERR;
                }
                break;
            case PAM_ERROR_MSG:
            case PAM_TEXT_INFO:
                break;
            default:
                for (int j = 0; j < i; j++) {
                    free((*response)[j].resp);
                }
                free(*response);
                *response = NULL;
                return PAM_CONV_ERR;
        }
    }
    return PAM_SUCCESS;
}
*/
import "C"

import (
	"bufio"
	"errors"
	"fmt"
	"os"
	"strconv"
	"strings"
	"syscall"
	"unsafe"
)

var (
	ErrAuthFailed     = errors.New("authentication failed")
	ErrPAMInit        = errors.New("failed to initialize PAM")
	ErrUserNotFound   = errors.New("user not found in system")
	ErrPrivDropFailed = errors.New("failed to drop privileges")
)

// UserInfo holds resolved Linux user information.
type UserInfo struct {
	Username string
	UID      uint32
	GID      uint32
	HomeDir  string
	Shell    string
	Groups   []uint32
}

// PAMAuthenticator handles PAM-based authentication.
type PAMAuthenticator struct {
	ServiceName string
}

// NewPAMAuthenticator creates a new PAM authenticator with the given service name.
// The service name should correspond to a file in /etc/pam.d/ (e.g., "clouddesk").
func NewPAMAuthenticator(serviceName string) *PAMAuthenticator {
	return &PAMAuthenticator{
		ServiceName: serviceName,
	}
}

// Authenticate validates a username/password pair against the system's PAM stack.
func (p *PAMAuthenticator) Authenticate(username, password string) (*UserInfo, error) {
	if username == "" || password == "" {
		return nil, fmt.Errorf("username and password must not be empty")
	}

	// Look up the user in /etc/passwd first to get UID/GID.
	passwd, err := syscall.Getpwnam(username)
	if err != nil {
		return nil, fmt.Errorf("%w: user '%s' not found in system passwd database", ErrUserNotFound, username)
	}

	// Perform PAM authentication via CGO.
	if err := p.pamAuthenticate(username, password); err != nil {
		return nil, fmt.Errorf("%w: PAM rejected credentials for '%s': %v", ErrAuthFailed, username, err)
	}

	// Resolve supplementary groups.
	gids, err := getSupplementaryGroups(username, passwd.Gid)
	if err != nil {
		// Non-fatal: log but don't fail authentication.
		gids = []uint32{passwd.Gid}
	}

	info := &UserInfo{
		Username: username,
		UID:      passwd.Uid,
		GID:      passwd.Gid,
		HomeDir:  passwd.Dir,
		Shell:    passwd.Shell,
		Groups:   gids,
	}

	return info, nil
}

// pamAuthenticate performs the actual PAM conversation.
func (p *PAMAuthenticator) pamAuthenticate(username, password string) error {
	cUsername := C.CString(username)
	defer C.free(unsafe.Pointer(cUsername))

	cPassword := C.CString(password)
	defer C.free(unsafe.Pointer(cPassword))

	cService := C.CString(p.ServiceName)
	defer C.free(unsafe.Pointer(cService))

	var pamh *C.pam_handle_t

	// Set up the conversation struct.
	conv := C.struct_pam_conv{
		conv:        C.pam_conv_func(C.pam_conv_cb),
		appdata_ptr: unsafe.Pointer(cPassword),
	}

	// Initialize PAM handle.
	ret := C.pam_start(cService, cUsername, &conv, &pamh)
	if ret != C.PAM_SUCCESS {
		return fmt.Errorf("%w: pam_start returned %d (%s)", ErrPAMInit, int(ret), C.GoString(C.pam_strerror(nil, ret)))
	}
	defer C.pam_end(pamh, ret)

	// Authenticate (PAM_AUTHENTICATE).
	ret = C.pam_authenticate(pamh, 0)
	if ret != C.PAM_SUCCESS {
		return fmt.Errorf("%w: pam_authenticate returned %d (%s)",
			ErrAuthFailed, int(ret), C.GoString(C.pam_strerror(pamh, ret)))
	}

	// Check account validity (expired, locked, etc.).
	ret = C.pam_acct_mgmt(pamh, 0)
	if ret != C.PAM_SUCCESS {
		return fmt.Errorf("%w: account check failed: %d (%s)",
			ErrAuthFailed, int(ret), C.GoString(C.pam_strerror(pamh, ret)))
	}

	return nil
}

// getSupplementaryGroups returns all GIDs for the given username and primary GID
// by parsing /etc/group. This is a pure Go implementation that avoids CGO
// complications with getgrouplist(3) which varies across glibc/musl.
func getSupplementaryGroups(username string, primaryGID uint32) ([]uint32, error) {
	gidSet := make(map[uint32]bool)
	gidSet[primaryGID] = true

	f, err := os.Open("/etc/group")
	if err != nil {
		return []uint32{primaryGID}, nil // non-fatal
	}
	defer f.Close()

	scanner := bufio.NewScanner(f)
	for scanner.Scan() {
		line := scanner.Text()
		// /etc/group format: groupname:x:gid:user1,user2,...
		parts := strings.SplitN(line, ":", 4)
		if len(parts) < 4 {
			continue
		}

		gidStr := strings.TrimSpace(parts[2])
		gid, err := strconv.ParseUint(gidStr, 10, 32)
		if err != nil {
			continue
		}

		members := strings.Split(parts[3], ",")
		for _, member := range members {
			if strings.TrimSpace(member) == username {
				gidSet[uint32(gid)] = true
				break
			}
		}
	}

	result := make([]uint32, 0, len(gidSet))
	for gid := range gidSet {
		result = append(result, gid)
	}
	return result, nil
}

// ResolveUser looks up a user by username from /etc/passwd without authentication.
func ResolveUser(username string) (*UserInfo, error) {
	passwd, err := syscall.Getpwnam(username)
	if err != nil {
		return nil, fmt.Errorf("%w: user '%s'", ErrUserNotFound, username)
	}

	gids, _ := getSupplementaryGroups(username, passwd.Gid)

	return &UserInfo{
		Username: username,
		UID:      passwd.Uid,
		GID:      passwd.Gid,
		HomeDir:  passwd.Dir,
		Shell:    passwd.Shell,
		Groups:   gids,
	}, nil
}

// ResolveUID looks up a user by UID.
func ResolveUID(uid uint32) (*UserInfo, error) {
	passwd, err := syscall.Getpwuid(uid)
	if err != nil {
		return nil, fmt.Errorf("%w: uid %d", ErrUserNotFound, uid)
	}

	gids, _ := getSupplementaryGroups(passwd.Name, passwd.Gid)

	return &UserInfo{
		Username: passwd.Name,
		UID:      passwd.Uid,
		GID:      passwd.Gid,
		HomeDir:  passwd.Dir,
		Shell:    passwd.Shell,
		Groups:   gids,
	}, nil
}

// DropPrivileges sets the effective UID/GID to the target user.
// The calling process must have CAP_SETUID/CAP_SETGID or be running as root.
// Returns a restore function that reverts to the original UID/GID.
func DropPrivileges(targetUID, targetGID uint32) (restore func() error, err error) {
	origUID := syscall.Geteuid()
	origGID := syscall.Getegid()

	// Set supplementary groups.
	if err := syscall.Setgroups([]int{}); err != nil {
		return nil, fmt.Errorf("%w: failed to clear supplementary groups: %v", ErrPrivDropFailed, err)
	}

	// Set GID first, then UID.
	if err := syscall.Setresgid(int(targetGID), int(targetGID), int(origGID)); err != nil {
		return nil, fmt.Errorf("%w: failed to set GID to %d: %v", ErrPrivDropFailed, targetGID, err)
	}
	if err := syscall.Setresuid(int(targetUID), int(targetUID), int(origUID)); err != nil {
		return nil, fmt.Errorf("%w: failed to set UID to %d: %v", ErrPrivDropFailed, targetUID, err)
	}

	// Verify.
	if syscall.Geteuid() != int(targetUID) {
		return nil, fmt.Errorf("%w: privilege drop verification failed (euid=%d, want=%d)",
			ErrPrivDropFailed, syscall.Geteuid(), targetUID)
	}

	restore = func() error {
		if err := syscall.Setresgid(int(origGID), int(origGID), int(origGID)); err != nil {
			return fmt.Errorf("failed to restore GID: %v", err)
		}
		if err := syscall.Setresuid(int(origUID), int(origUID), int(origUID)); err != nil {
			return fmt.Errorf("failed to restore UID: %v", err)
		}
		return nil
	}

	return restore, nil
}

// HomePath returns the absolute home directory path for a username.
func HomePath(username string) (string, error) {
	passwd, err := syscall.Getpwnam(username)
	if err != nil {
		return "", fmt.Errorf("failed to resolve home directory for '%s': %v", username, err)
	}
	return passwd.Dir, nil
}

// isRunningAsRoot checks if the current process has root privileges.
func isRunningAsRoot() bool {
	return syscall.Geteuid() == 0
}

// RequireRoot panics if not running as root. Call this at startup.
func RequireRoot() {
	if !isRunningAsRoot() {
		panic("CloudDesk OS backend must be started as root (UID 0) for PAM authentication and privilege dropping. Use systemd with User=root or run with sudo.")
	}
}
