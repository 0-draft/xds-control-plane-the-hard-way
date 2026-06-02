.PHONY: build up down smoke logs cp-logs envoy-logs clean check fmt push-good push-broken status

# Build all images without starting anything.
build:
	docker compose build

# Start the full stack in the background.
up:
	docker compose up -d --build

# Stop and remove containers + network.
down:
	docker compose down -v

# Tail the control plane logs (SUBSCRIBE / ACK / NACK / rollback appear here).
logs cp-logs:
	docker compose logs -f controlplane

envoy-logs:
	docker compose logs -f envoy

# Admin API shortcuts.
push-good:
	curl -sS -X POST http://localhost:19000/push/good

push-broken:
	curl -sS -X POST http://localhost:19000/push/broken

status:
	curl -sS http://localhost:19000/status

# End-to-end check: converge on v1, push a broken Listener, prove traffic
# survives the NACK via rollback, then push a clean version swap.
smoke:
	@echo "==> Waiting for Envoy to converge on v1..."
	@ok=0; for i in $$(seq 1 30); do \
		if curl -sf -o /dev/null http://localhost:10000/; then ok=1; break; fi; \
		sleep 1; \
	done; \
	if [ "$$ok" -ne 1 ]; then echo "FAIL: Envoy did not converge"; docker compose logs --tail=80 controlplane envoy; exit 1; fi; \
	echo "    OK: serving v1"
	@echo "==> Pushing a broken Listener (empty stat_prefix)..."
	@curl -sS -X POST http://localhost:19000/push/broken
	@sleep 3
	@echo "==> Traffic must still flow (Envoy NACKed, control plane rolled back)..."
	@if curl -sf -o /dev/null http://localhost:10000/; then echo "    OK: still serving after the broken push"; else \
		echo "FAIL: traffic broke after broken push"; docker compose logs --tail=120 controlplane envoy; exit 1; fi
	@echo "==> Control plane must have logged a NACK and a rollback..."
	@docker compose logs --no-log-prefix controlplane > /tmp/hardway-cp.log 2>&1; \
	if grep -q "NACK" /tmp/hardway-cp.log && grep -q "rolling back after NACK" /tmp/hardway-cp.log; then \
		echo "    OK: NACK observed, rolled back to last-known-good"; \
	else \
		echo "FAIL: no NACK / rollback in control plane log"; cat /tmp/hardway-cp.log; exit 1; fi
	@echo "==> Pushing a clean version swap (good)..."
	@curl -sS -X POST http://localhost:19000/push/good
	@sleep 3
	@if curl -sf -o /dev/null http://localhost:10000/; then echo "    OK: serving after the good swap"; else \
		echo "FAIL: traffic broke after good push"; docker compose logs --tail=120 controlplane envoy; exit 1; fi
	@echo "==> Final status:"; curl -sS http://localhost:19000/status
	@echo "SMOKE PASSED"

# Cargo-side checks (fast, no docker).
check:
	cargo check --workspace

fmt:
	cargo fmt --all

clean:
	docker compose down -v --rmi local
	cargo clean
