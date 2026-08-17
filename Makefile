.PHONY: bootstrap build check dev migrate test

bootstrap:
	cd apps/web && npm install

build:
	cd apps/web && npm run build
	cargo build --workspace

check:
	cargo fmt --all -- --check
	cargo clippy --workspace --all-targets -- -D warnings
	cd apps/web && npm run lint

test:
	cargo test --workspace
	cd apps/web && npm test

migrate: local-secrets
	cargo run -p clouddeskd -- migrate --config config/clouddesk.toml

local-secrets:
	mkdir -p var/keys
	test -s var/keys/master.key || openssl rand 32 > var/keys/master.key
	test -s var/bootstrap.secret || openssl rand -base64 32 > var/bootstrap.secret

dev: local-secrets
	cargo run -p clouddeskd -- serve --config config/clouddesk.toml
