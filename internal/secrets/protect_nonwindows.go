//go:build !windows

package secrets

import "encoding/base64"

func protect(value []byte) ([]byte, error) {
	destination := make([]byte, base64.StdEncoding.EncodedLen(len(value)))
	base64.StdEncoding.Encode(destination, value)
	return destination, nil
}

func unprotect(value []byte) ([]byte, error) {
	destination := make([]byte, base64.StdEncoding.DecodedLen(len(value)))
	written, err := base64.StdEncoding.Decode(destination, value)
	return destination[:written], err
}
