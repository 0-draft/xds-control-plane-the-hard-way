.PHONY: build up down smoke logs cp-logs envoy-logs clean check fmt bump status

# Build all images without starting anything.
build:
	docker compose build

# Start the full stack in the background.
up:
	docker compose up -d --build

# Stop and remove containers + network.
down:
	docker compose down -v

# Tail the control plane logs (delta SUBSCRIBE / ACK / push appear here).
logs cp-logs:
	docker compose logs -f controlplane

envoy-logs:
	docker compose logs -f envoy

# Admin API shortcuts.
bump:
	curl -sS -X POST http://localhost:19000/bump

status:
	curl -sS http://localhost:19000/status

# End-to-end check: converge over Delta, bump only the route, and prove the
# push carried exactly one (RDS) resource while the header flips v1 -> v2.
smoke:
	@echo "==> Waiting for Envoy (Delta) to converge..."
	@ok=0; for i in $$(seq 1 30); do \
		if curl -sf -o /dev/null http://localhost:10000/; then ok=1; break; fi; \
		sleep 1; \
	done; \
	if [ "$$ok" -ne 1 ]; then echo "FAIL: Envoy did not converge"; docker compose logs --tail=80 controlplane envoy; exit 1; fi; \
	echo "    OK: serving"
	@v1=$$(curl -si http://localhost:10000/ | tr -d '\r' | awk -F': ' 'tolower($$1)=="x-config-version"{print $$2}'); \
	echo "    x-config-version before: $$v1"
	@echo "==> Bumping only the RouteConfiguration..."
	@curl -sS -X POST http://localhost:19000/bump
	@sleep 3
	@v2=$$(curl -si http://localhost:10000/ | tr -d '\r' | awk -F': ' 'tolower($$1)=="x-config-version"{print $$2}'); \
	echo "    x-config-version after:  $$v2"; \
	if [ "$$v2" = "v2" ]; then echo "    OK: route updated to v2"; else echo "FAIL: header did not change"; exit 1; fi
	@echo "==> The bump must have pushed exactly one (RDS) resource..."
	@docker compose logs --no-log-prefix controlplane > /tmp/hardway-cp4.log 2>&1; \
	if grep -q 'pushing delta ty="RDS" resources=1' /tmp/hardway-cp4.log; then \
		echo "    OK: the delta carried only RDS (incremental)"; \
	else \
		echo "FAIL: expected a single-resource RDS delta"; cat /tmp/hardway-cp4.log; exit 1; fi
	@echo "SMOKE PASSED"

# Cargo-side checks (fast, no docker).
check:
	cargo check --workspace

fmt:
	cargo fmt --all

clean:
	docker compose down -v --rmi local
	cargo clean
