.PHONY: bridge bridge-stop bridge-build bridge-logs bridge-status generate

BRIDGE_IMAGE   ?= cyclone-dds-ws-bridge
BRIDGE_NAME    ?= dds-ws-bridge
BRIDGE_PORT    ?= 9876
DDS_DOMAIN     ?= 0

# ---------------------------------------------------------------------------
# Bridge lifecycle
# ---------------------------------------------------------------------------

# Build the bridge Docker image from the repository root.
bridge-build:
	docker build -t $(BRIDGE_IMAGE) ..

# Start the bridge in the background.
# Requires: bridge-build (run manually; not a dependency to avoid slow rebuilds).
bridge:
	@docker inspect $(BRIDGE_NAME) >/dev/null 2>&1 \
		&& echo "bridge is already running ($(BRIDGE_NAME))" && exit 1 || true
	docker run -d --rm \
		--name $(BRIDGE_NAME) \
		-p $(BRIDGE_PORT):9876 \
		-e DDS_BRIDGE_WS_ADDR=0.0.0.0 \
		-e DDS_BRIDGE_WS_PORT=9876 \
		-e DDS_BRIDGE_DDS_DOMAIN=$(DDS_DOMAIN) \
		-e RUST_LOG=info \
		$(BRIDGE_IMAGE)
	@echo "bridge started on ws://localhost:$(BRIDGE_PORT)"

# Stop the running bridge container.
bridge-stop:
	docker stop $(BRIDGE_NAME)

# Show whether the bridge container is running.
bridge-status:
	@docker inspect -f '{{.State.Status}}' $(BRIDGE_NAME) 2>/dev/null || echo "not running"

# Tail bridge container logs.
bridge-logs:
	docker logs -f $(BRIDGE_NAME)

# ---------------------------------------------------------------------------
# Code generation
# ---------------------------------------------------------------------------

# Regenerate protobuf code (requires buf: https://buf.build/docs/installation).
generate:
	cd protobuf && buf generate proto
