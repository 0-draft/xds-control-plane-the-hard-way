.PHONY: build up down smoke logs cp-logs envoy-logs clean check fmt status

build:
	docker compose build

up:
	docker compose up -d --build

down:
	docker compose down -v

logs cp-logs:
	docker compose logs -f controlplane

envoy-logs:
	docker compose logs -f envoy

status:
	curl -sS http://localhost:19000/status

# End-to-end check: Envoy bootstraps the LDS/CDS collections by xdstp glob,
# follows xdstp singletons for RDS/EDS, and converges across two authorities.
smoke:
	@echo "==> Waiting for Envoy to converge over xdstp..."
	@ok=0; for i in $$(seq 1 40); do \
		if curl -sf -o /dev/null http://localhost:10000/; then ok=1; break; fi; \
		sleep 1; \
	done; \
	if [ "$$ok" -ne 1 ]; then echo "FAIL: Envoy did not converge"; docker compose logs --tail=120 controlplane envoy; exit 1; fi; \
	echo "    OK: serving"
	@echo "==> Body:"; curl -sS http://localhost:10000/ | sed 's/^/    /'
	@echo "==> The control plane pushed resources under both authorities:"
	@docker compose logs --no-log-prefix controlplane > /tmp/hardway-cp5.log 2>&1; \
	if grep -q 'pushing delta ty="LDS"' /tmp/hardway-cp5.log \
		&& grep -q 'pushing delta ty="CDS"' /tmp/hardway-cp5.log \
		&& grep -q 'pushing delta ty="RDS"' /tmp/hardway-cp5.log \
		&& grep -q 'pushing delta ty="EDS"' /tmp/hardway-cp5.log; then \
		echo "    OK: LDS+RDS (hardway) and CDS+EDS (edge) all served"; \
	else \
		echo "FAIL: not all xdstp resources were pushed"; cat /tmp/hardway-cp5.log; exit 1; fi
	@echo "==> Authority map:"; curl -sS http://localhost:19000/status | sed 's/^/    /'
	@echo "SMOKE PASSED"

check:
	cargo check --workspace

fmt:
	cargo fmt --all

clean:
	docker compose down -v --rmi local
	cargo clean
