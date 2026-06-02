.PHONY: build up down smoke logs cp-logs envoy-logs clean check fmt rotate status

# Build all images without starting anything.
build:
	docker compose build

# Start the full stack in the background.
up:
	docker compose up -d --build

# Stop and remove containers + network.
down:
	docker compose down -v

# Tail the control plane logs (SUBSCRIBE / ACK / SDS push appear here).
logs cp-logs:
	docker compose logs -f controlplane

envoy-logs:
	docker compose logs -f envoy

# Admin API shortcuts.
rotate:
	curl -sS -X POST http://localhost:19000/rotate

status:
	curl -sS http://localhost:19000/status

# End-to-end check: bring up TLS via SDS, then rotate the cert and prove the
# fingerprint changed on a live listener (no Envoy restart).
smoke:
	@echo "==> Waiting for Envoy to serve TLS on :10000..."
	@ok=0; for i in $$(seq 1 30); do \
		if curl -ksf -o /dev/null https://localhost:10000/; then ok=1; break; fi; \
		sleep 1; \
	done; \
	if [ "$$ok" -ne 1 ]; then echo "FAIL: TLS endpoint did not come up"; docker compose logs --tail=80 controlplane envoy; exit 1; fi; \
	echo "    OK: HTTPS serving (cert delivered over SDS)"
	@echo "==> Body over TLS:"; curl -ksS https://localhost:10000/ | sed 's/^/    /'
	@echo "==> Rotating the certificate on a live listener..."; \
	fp1=$$(echo | openssl s_client -connect localhost:10000 -servername localhost 2>/dev/null | openssl x509 -noout -fingerprint -sha256); \
	echo "    before: $$fp1"; \
	curl -sS -X POST http://localhost:19000/rotate; \
	sleep 3; \
	fp2=$$(echo | openssl s_client -connect localhost:10000 -servername localhost 2>/dev/null | openssl x509 -noout -fingerprint -sha256); \
	echo "    after:  $$fp2"; \
	if [ "$$fp1" = "$$fp2" ]; then echo "FAIL: certificate did not change"; docker compose logs --tail=80 controlplane; exit 1; fi; \
	echo "    OK: certificate hot-rotated without a restart"
	@if curl -ksf -o /dev/null https://localhost:10000/; then echo "==> OK: still serving after rotation"; else echo "FAIL"; exit 1; fi
	@echo "SMOKE PASSED"

# Cargo-side checks (fast, no docker).
check:
	cargo check --workspace

fmt:
	cargo fmt --all

clean:
	docker compose down -v --rmi local
	cargo clean
