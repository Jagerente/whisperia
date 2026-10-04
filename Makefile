POWERSHELL := powershell -NoProfile -ExecutionPolicy Bypass

.PHONY: all dist pictures build test check clean docker docker-up docker-down

all: dist

dist:
	$(POWERSHELL) -File scripts/package.ps1

pictures:
	$(POWERSHELL) -File scripts/pictures.ps1

build:
	cargo build --release

test:
	cargo test --workspace

check:
	cargo fmt --all -- --check
	cargo clippy --workspace --all-targets -- -D warnings
	cargo test --workspace

docker:
	docker build -t whisperia .

docker-up:
	docker compose up -d

docker-down:
	docker compose down

clean:
	cargo clean
