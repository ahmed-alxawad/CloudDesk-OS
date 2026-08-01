package crypto

import (
	"crypto/aes"
	"crypto/cipher"
	"crypto/rand"
	"encoding/base64"
	"errors"
	"fmt"
	"io"

	"golang.org/x/crypto/argon2"
)

var (
	ErrInvalidKeyLength  = errors.New("key must be exactly 32 bytes for AES-256")
	ErrInvalidCiphertext = errors.New("invalid ciphertext")
	ErrEncryptionFailed  = errors.New("encryption failed")
	ErrDecryptionFailed   = errors.New("decryption failed")
)

// AES256GCM provides AES-256-GCM authenticated encryption.
type AES256GCM struct {
	key  []byte
	aead cipher.AEAD
}

// NewAES256GCM creates a new AES-256-GCM encryptor from a raw key.
// The key must be exactly 32 bytes.
func NewAES256GCM(key []byte) (*AES256GCM, error) {
	if len(key) != 32 {
		return nil, fmt.Errorf("%w: got %d bytes, need 32", ErrInvalidKeyLength, len(key))
	}

	block, err := aes.NewCipher(key)
	if err != nil {
		return nil, fmt.Errorf("%w: %v", ErrEncryptionFailed, err)
	}

	aead, err := cipher.NewGCM(block)
	if err != nil {
		return nil, fmt.Errorf("%w: %v", ErrEncryptionFailed, err)
	}

	return &AES256GCM{
		key:  key,
		aead: aead,
	}, nil
}

// NewAES256GCMFromBase64 creates an AES-256-GCM encryptor from a base64-encoded key.
func NewAES256GCMFromBase64(encodedKey string) (*AES256GCM, error) {
	key, err := base64.StdEncoding.DecodeString(encodedKey)
	if err != nil {
		return nil, fmt.Errorf("failed to decode base64 key: %w", err)
	}
	return NewAES256GCM(key)
}

// Encrypt encrypts plaintext using AES-256-GCM.
// Returns base64-encoded ciphertext (nonce + ciphertext + tag).
func (a *AES256GCM) Encrypt(plaintext []byte) (string, error) {
	nonce := make([]byte, a.aead.NonceSize())
	if _, err := io.ReadFull(rand.Reader, nonce); err != nil {
		return "", fmt.Errorf("%w: failed to generate nonce: %v", ErrEncryptionFailed, err)
	}

	// Seal appends the ciphertext and tag to nonce.
	ciphertext := a.aead.Seal(nonce, nonce, plaintext, nil)

	return base64.StdEncoding.EncodeToString(ciphertext), nil
}

// Decrypt decrypts base64-encoded ciphertext.
// The input must be the same base64 format produced by Encrypt (nonce + ciphertext + tag).
func (a *AES256GCM) Decrypt(encodedCiphertext string) ([]byte, error) {
	ciphertext, err := base64.StdEncoding.DecodeString(encodedCiphertext)
	if err != nil {
		return nil, fmt.Errorf("%w: failed to decode base64 ciphertext: %v", ErrDecryptionFailed, err)
	}

	nonceSize := a.aead.NonceSize()
	if len(ciphertext) < nonceSize {
		return nil, fmt.Errorf("%w: ciphertext too short", ErrInvalidCiphertext)
	}

	nonce, encryptedData := ciphertext[:nonceSize], ciphertext[nonceSize:]

	plaintext, err := a.aead.Open(nil, nonce, encryptedData, nil)
	if err != nil {
		return nil, fmt.Errorf("%w: %v", ErrDecryptionFailed, err)
	}

	return plaintext, nil
}

// EncryptString is a convenience method to encrypt a string.
func (a *AES256GCM) EncryptString(plaintext string) (string, error) {
	return a.Encrypt([]byte(plaintext))
}

// DecryptString is a convenience method to decrypt to a string.
func (a *AES256GCM) DecryptString(encodedCiphertext string) (string, error) {
	data, err := a.Decrypt(encodedCiphertext)
	if err != nil {
		return "", err
	}
	return string(data), nil
}

// GenerateKey creates a new random 32-byte AES-256 key.
// Returns the key as a base64-encoded string.
func GenerateKey() (string, error) {
	key := make([]byte, 32)
	if _, err := io.ReadFull(rand.Reader, key); err != nil {
		return "", fmt.Errorf("failed to generate random key: %w", err)
	}
	return base64.StdEncoding.EncodeToString(key), nil
}

// DeriveKey derives a 32-byte key from a passphrase using argon2id.
func DeriveKey(passphrase string, salt []byte) []byte {
	return argon2.IDKey([]byte(passphrase), salt, 1, 64*1024, 4, 32)
}
