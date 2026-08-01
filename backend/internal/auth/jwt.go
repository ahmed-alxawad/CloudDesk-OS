package auth

import (
	"fmt"
	"time"

	"github.com/clouddesk-os/backend/pkg/models"
	"github.com/golang-jwt/jwt/v5"
)

// JWTManager handles JWT token creation and validation.
type JWTManager struct {
	secret          []byte
	expirationHours int
}

// NewJWTManager creates a new JWT manager with the given secret and expiration.
func NewJWTManager(secret string, expirationHours int) *JWTManager {
	return &JWTManager{
		secret:          []byte(secret),
		expirationHours: expirationHours,
	}
}

// Claims represents the custom JWT claims.
type Claims struct {
	UserID   int64  `json:"uid"`
	Username string `json:"username"`
	UID      uint32 `json:"os_uid"`
	GID      uint32 `json:"os_gid"`
	Role     string `json:"role"`
	jwt.RegisteredClaims
}

// GenerateToken creates a signed JWT for the given user.
func (m *JWTManager) GenerateToken(user *models.User) (string, int64, error) {
	now := time.Now()
	expiresAt := now.Add(time.Duration(m.expirationHours) * time.Hour)

	claims := &Claims{
		UserID:   user.ID,
		Username: user.Username,
		UID:      user.UID,
		GID:      user.GID,
		Role:     user.Role,
		RegisteredClaims: jwt.RegisteredClaims{
			IssuedAt:  jwt.NewNumericDate(now),
			ExpiresAt: jwt.NewNumericDate(expiresAt),
			NotBefore: jwt.NewNumericDate(now),
			Issuer:    "clouddesk-os",
			Subject:   user.Username,
		},
	}

	token := jwt.NewWithClaims(jwt.SigningMethodHS512, claims)
	tokenString, err := token.SignedString(m.secret)
	if err != nil {
		return "", 0, fmt.Errorf("failed to sign JWT: %w", err)
	}

	return tokenString, expiresAt.Unix(), nil
}

// ValidateToken parses and validates a JWT string, returning the claims.
func (m *JWTManager) ValidateToken(tokenString string) (*Claims, error) {
	token, err := jwt.ParseWithClaims(tokenString, &Claims{}, func(token *jwt.Token) (interface{}, error) {
		if _, ok := token.Method.(*jwt.SigningMethodHMAC); !ok {
			return nil, fmt.Errorf("unexpected signing method: %v", token.Header["alg"])
		}
		return m.secret, nil
	})

	if err != nil {
		return nil, fmt.Errorf("invalid token: %w", err)
	}

	claims, ok := token.Claims.(*Claims)
	if !ok || !token.Valid {
		return nil, fmt.Errorf("invalid token claims")
	}

	return claims, nil
}

// RefreshToken generates a new token if the current one is close to expiring.
func (m *JWTManager) RefreshToken(oldToken string) (string, int64, error) {
	claims, err := m.ValidateToken(oldToken)
	if err != nil {
		return "", 0, err
	}

	now := time.Now()
	// Allow refresh if token has less than 25% of its lifespan remaining.
	timeLeft := claims.ExpiresAt.Time.Sub(now)
	totalLife := time.Duration(m.expirationHours) * time.Hour
	if timeLeft > totalLife/4 {
		return oldToken, claims.ExpiresAt.Unix(), nil
	}

	newUser := &models.User{
		ID:       claims.UserID,
		Username: claims.Username,
		UID:      claims.UID,
		GID:      claims.GID,
		Role:     claims.Role,
	}
	return m.GenerateToken(newUser)
}
