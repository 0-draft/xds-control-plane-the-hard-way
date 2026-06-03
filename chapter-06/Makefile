.PHONY: build up down smoke logs cp-logs envoy-logs clean check fmt dist

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

# Sample the backend distribution over 200 requests.
dist:
	@a=0; b=0; for i in $$(seq 1 200); do \
		u=$$(curl -s -D - -o /dev/null http://localhost:10000/ | tr -d '\r' | awk -F': ' 'tolower($$1)=="x-upstream"{print $$2}'); \
		if [ "$$u" = "upstream-a" ]; then a=$$((a+1)); elif [ "$$u" = "upstream-b" ]; then b=$$((b+1)); fi; \
	done; \
	echo "upstream-a (busy 0.9): $$a    upstream-b (idle 0.1): $$b"

# End-to-end check: converge, warm up so Envoy has ORCA reports + measured QPS,
# then prove the idle backend (lower utilization) gets the majority of traffic.
smoke:
	@echo "==> Waiting for Envoy to converge..."
	@ok=0; for i in $$(seq 1 40); do \
		if curl -sf -o /dev/null http://localhost:10000/; then ok=1; break; fi; \
		sleep 1; \
	done; \
	if [ "$$ok" -ne 1 ]; then echo "FAIL: Envoy did not converge"; docker compose logs --tail=120 controlplane envoy; exit 1; fi; \
	echo "    OK: serving"
	@echo "==> Warming up so OOB ORCA reports and measured QPS settle..."
	@for i in $$(seq 1 150); do curl -s -o /dev/null http://localhost:10000/; done
	@sleep 4
	@echo "==> Sampling 200 requests across the two backends..."
	@a=0; b=0; for i in $$(seq 1 200); do \
		u=$$(curl -s -D - -o /dev/null http://localhost:10000/ | tr -d '\r' | awk -F': ' 'tolower($$1)=="x-upstream"{print $$2}'); \
		if [ "$$u" = "upstream-a" ]; then a=$$((a+1)); elif [ "$$u" = "upstream-b" ]; then b=$$((b+1)); fi; \
	done; \
	echo "    upstream-a (busy 0.9): $$a    upstream-b (idle 0.1): $$b"; \
	if [ "$$b" -gt "$$a" ]; then \
		echo "    OK: the lighter backend received more traffic (ORCA-weighted)"; \
	else \
		echo "FAIL: load was not shifted toward the idle backend"; docker compose logs --tail=60 controlplane; exit 1; fi
	@echo "SMOKE PASSED"

check:
	cargo check --workspace

fmt:
	cargo fmt --all

clean:
	docker compose down -v --rmi local
	cargo clean
