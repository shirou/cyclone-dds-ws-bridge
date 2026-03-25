IMAGE_NAME := cyclone-dds-ws-bridge
TAG        ?= latest

.PHONY: build run stop clean fixtures

build:
	docker build -t $(IMAGE_NAME):$(TAG) .

run:
	docker run -d --name $(IMAGE_NAME) -p 9876:9876 $(IMAGE_NAME):$(TAG)

stop:
	docker stop $(IMAGE_NAME) && docker rm $(IMAGE_NAME)

clean:
	docker rmi $(IMAGE_NAME):$(TAG)

# Regenerate test fixtures inside Docker (CycloneDDS required).
# Run this after protocol changes, then commit the updated .bin files.
fixtures:
	docker build --target generate-fixtures -t $(IMAGE_NAME)-fixtures .
	docker run --rm -v $(CURDIR)/tests/fixtures:/out $(IMAGE_NAME)-fixtures \
		sh -c 'cp /app/tests/fixtures/*.bin /out/'
