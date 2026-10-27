SERVICE  := honeypot-bot
UNIT_DIR := /etc/systemd/system
DIR      := $(CURDIR)
USER     ?= $(shell id -un)

.PHONY: help not-root build run test install uninstall start stop restart status logs

help:
	@echo "make build      Build the release binary"
	@echo "make run        Build and run the bot in the foreground"
	@echo "make test       Run the tests"
	@echo "make install    Build, install and enable the systemd service (starts on boot)"
	@echo "make uninstall  Stop, disable and remove the systemd service"
	@echo "make start|stop|restart|status"
	@echo "make logs       Follow the service logs"

# The Makefile calls sudo itself where needed. Running it under sudo would
# lose cargo from PATH and install the service to run as root.
not-root:
	@if [ "$$(id -u)" = 0 ]; then echo "Run 'make $(MAKECMDGOALS)' without sudo; it asks for sudo when needed."; exit 1; fi

build:
	cargo build --release

run: build
	./target/release/$(SERVICE)

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

# Rebuild before restarting so code changes take effect
restart: not-root build
	sudo systemctl restart $(SERVICE)

start stop:
	sudo systemctl $@ $(SERVICE)

status:
	systemctl status $(SERVICE)

logs:
	journalctl -u $(SERVICE) -f -o cat
