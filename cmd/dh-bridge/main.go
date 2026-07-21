package main

import (
	"context"
	"flag"
	"fmt"
	"log"
	"os"
	"os/signal"

	"dh/internal/embeddedbridge"
)

func main() {
	host := flag.String("host", "127.0.0.1", "bridge listen host")
	port := flag.Int("port", 51235, "bridge listen port")
	devtools := flag.String("devtools", "http://127.0.0.1:9222", "WangShangLiao DevTools URL")
	flag.Parse()

	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt)
	defer stop()
	service, err := embeddedbridge.Start(ctx, embeddedbridge.Options{
		Address: fmt.Sprintf("%s:%d", *host, *port), DevToolsURL: *devtools,
	})
	if err != nil {
		log.Fatal(err)
	}
	defer service.Close()
	log.Printf("DH Bridge: %s (reused=%t)", service.URL, service.Reused)
	log.Printf("WangShangLiao DevTools: %s", *devtools)
	if err := service.Wait(); err != nil {
		log.Fatal(err)
	}
}
