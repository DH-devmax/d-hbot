//go:build !windows

package main

import "fmt"

func main() {
	fmt.Println("DH desktop targets Windows. Build with: GOOS=windows GOARCH=amd64 go build ./cmd/dh")
}
