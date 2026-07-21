.PHONY: test build bridge smoke-tool resources manual-pdf package clean

GO ?= go
PYTHON ?= python3
RSRC ?= $(GO) run github.com/akavel/rsrc@v0.10.2
VERSION := 2.7.0
PRODUCT := DH-BOT
SIGNTOOL ?= signtool
SIGN_TIMESTAMP_URL ?= http://timestamp.digicert.com
OUT := build/DH-BOT.exe
BRIDGE_OUT := build/tools/diagnostic/DHBridge.exe
DIST_NAME := DH-BOT-$(VERSION)-windows-x64
DIST_DIR := dist/$(DIST_NAME)
DIAGNOSTIC_DIR := $(DIST_DIR)/tools/diagnostic
SINGLE_EXE := dist/$(DIST_NAME).exe
MANUAL_PDF := output/pdf/DH-Manual-ZH.pdf
DIST_MANUAL_PDF := dist/DH-Manual-ZH.pdf

test:
	$(GO) test ./...

resources:
	set -e; tmp=$$(mktemp -d); \
	$(RSRC) \
		-arch amd64 \
		-ico assets/icon.ico,assets/tray.ico \
		-manifest cmd/dh/dh.exe.manifest \
		-o "$$tmp/rsrc_windows_amd64.syso"; \
	mv "$$tmp/rsrc_windows_amd64.syso" cmd/dh/rsrc_windows_amd64.syso; \
	rmdir "$$tmp"

build: test resources
	mkdir -p build
	GOOS=windows GOARCH=amd64 CGO_ENABLED=0 $(GO) build \
		-trimpath -ldflags="-s -w -H=windowsgui" -o $(OUT) ./cmd/dh
	@if [ -n "$(SIGN_CERT_SHA1)" ]; then \
		$(SIGNTOOL) sign /sha1 "$(SIGN_CERT_SHA1)" /fd SHA256 /tr "$(SIGN_TIMESTAMP_URL)" /td SHA256 "$(OUT)"; \
	fi

bridge:
	mkdir -p build/tools/diagnostic
	GOOS=windows GOARCH=amd64 CGO_ENABLED=0 $(GO) build \
		-trimpath -ldflags="-s -w" -o $(BRIDGE_OUT) ./cmd/dh-bridge

smoke-tool:
	mkdir -p build
	GOOS=windows GOARCH=amd64 CGO_ENABLED=0 $(GO) build \
		-trimpath -o build/DH-smoke.exe ./cmd/dh-smoke

manual-pdf:
	$(PYTHON) tools/build_manual_pdf.py $(MANUAL_PDF)
	mkdir -p dist
	cp $(MANUAL_PDF) $(DIST_MANUAL_PDF)

package: build bridge manual-pdf
	rm -rf $(DIST_DIR)
	mkdir -p $(DIST_DIR) $(DIAGNOSTIC_DIR)
	cp build/DH-BOT.exe package/Start-DH.cmd package/Start-DH.vbs package/README.txt package/ZCG-Compatible-Rules.json AGENTS.md assets/icon.svg assets/logo.png assets/tray.ico $(DIST_DIR)/
	cp build/DH-BOT.exe $(SINGLE_EXE)
	cp $(BRIDGE_OUT) $(DIAGNOSTIC_DIR)/
	cp docs/DH使用手册.md $(DIST_DIR)/DH-Manual-ZH.md
	cp $(DIST_MANUAL_PDF) $(DIST_DIR)/DH-Manual-ZH.pdf
	cd $(DIST_DIR) && shasum -a 256 DH-BOT.exe tools/diagnostic/DHBridge.exe Start-DH.cmd Start-DH.vbs README.txt ZCG-Compatible-Rules.json AGENTS.md icon.svg logo.png tray.ico DH-Manual-ZH.md DH-Manual-ZH.pdf > SHA256SUMS.txt
	cd dist && shasum -a 256 $(DIST_NAME).exe > $(DIST_NAME).exe.sha256
	cd dist && zip -qr -FS $(DIST_NAME).zip $(DIST_NAME)
	cd dist && shasum -a 256 $(DIST_NAME).zip > $(DIST_NAME).zip.sha256

clean:
	rm -rf build
