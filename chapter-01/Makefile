.PHONY: build up down smoke logs cp-logs envoy-logs clean check fmt

# Build all images without starting anything.
build:
	docker compose build

# Start the full stack in the background.
up:
	docker compose up -d --build

# Stop and remove containers + network.
down:
	docker compose down -v

# Tail the control plane logs (SUBSCRIBE / ACK / NACK appear here).
logs cp-logs:
	docker compose logs -f controlplane

envoy-logs:
	docker compose logs -f envoy

# End-to-end check. Waits up to ~30s for Envoy to converge on xDS.
smoke:
	@echo "Waiting for Envoy to come up..."
	@for i in $$(seq 1 30); do \
		if curl -sf -o /dev/null http://localhost:10000/; then \
			echo "OK: Envoy is serving traffic"; \
			curl -sS http://localhost:10000/; \
			exit 0; \
		fi; \
		sleep 1; \
	done; \
	echo "FAIL: Envoy did not start serving in time"; \
	docker compose logs --tail=80 controlplane envoy; \
	exit 1

# Cargo-side checks (fast, no docker).
check:
	cargo check --workspace

fmt:
	cargo fmt --all

clean:
	docker compose down -v --rmi local
	cargo clean
