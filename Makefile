SERVICE  := honeypot-bot
UNIT_DIR := /etc/systemd/system
DIR      := $(CURDIR)
USER     ?= $(shell id -un)
BIN      := target/release/$(SERVICE)

# Static (musl) build used for `make dist`, runs on any x86_64 Linux
MUSL     := x86_64-unknown-linux-musl
VERSION  := $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml 2>/dev/null | head -1)
PKG      := $(SERVICE)-$(VERSION)-x86_64-linux

.PHONY: help not-root build run test install uninstall start stop restart status logs dist

help:
	@echo "make build      Build the release binary"
	@echo "make run        Build and run the bot in the foreground"
	@echo "make test       Run the tests"
	@echo "make install    Build, install and enable the systemd service (starts on boot)"
	@echo "make uninstall  Stop, disable and remove the systemd service"
	@echo "make start|stop|restart|status"
	@echo "make logs       Follow the service logs"
	@echo "make dist       Package a static binary for other servers into dist/"

# The Makefile calls sudo itself where needed. Running it under sudo would
# lose cargo from PATH and install the service to run as root.
not-root:
	@if [ "$$(id -u)" = 0 ]; then echo "Run 'make $(MAKECMDGOALS)' without sudo; it asks for sudo when needed."; exit 1; fi

# In a `make dist` package there is no Cargo.toml: use the bundled binary.
build:
ifneq ($(wildcard Cargo.toml),)
	cargo build --release
else
	@test -x $(BIN) || { echo "Missing $(BIN)"; exit 1; }
endif

run: build
	./$(BIN)

test:
	cargo test

install: not-root build
	@test -f .env || { echo "Missing .env (copy .env.example and add DISCORD_TOKEN)"; exit 1; }
	mkdir -p database
	sed -e 's|@DIR@|$(DIR)|g' -e 's|@USER@|$(USER)|g' $(SERVICE).service.in > $(SERVICE).service
	sudo install -m 644 $(SERVICE).service $(UNIT_DIR)/$(SERVICE).service
	rm -f $(SERVICE).service
	sudo systemctl daemon-reload
	sudo systemctl enable --now $(SERVICE)

uninstall:
	-sudo systemctl disable --now $(SERVICE)
	sudo rm -f $(UNIT_DIR)/$(SERVICE).service
	sudo systemctl daemon-reload

# Packages a static binary with the Makefile and service template, so on
# another server: extract, add .env, then `make install` (no Rust needed).
dist:
	@test -d "$$(rustc --print sysroot)/lib/rustlib/$(MUSL)" && command -v musl-gcc >/dev/null || \
		{ echo "Missing musl toolchain: sudo pacman -S rust-musl musl"; exit 1; }
	cargo build --release --target $(MUSL)
	rm -rf dist/$(PKG)
	mkdir -p dist/$(PKG)/target/release
	install -m 755 target/$(MUSL)/release/$(SERVICE) dist/$(PKG)/$(BIN)
	cp Makefile $(SERVICE).service.in .env.example README.md dist/$(PKG)/
	tar -czf dist/$(PKG).tar.gz -C dist $(PKG)
	rm -rf dist/$(PKG)
	@echo "Created dist/$(PKG).tar.gz"

# Rebuild before restarting so code changes take effect
restart: not-root build
	sudo systemctl restart $(SERVICE)

start stop:
	sudo systemctl $@ $(SERVICE)

status:
	systemctl status $(SERVICE)

logs:
	journalctl -u $(SERVICE) -f -o cat
