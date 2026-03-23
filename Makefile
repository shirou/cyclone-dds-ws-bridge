IMAGE_NAME := cyclone-dds-ws-bridge
TAG        ?= latest

.PHONY: build run stop clean

build:
	docker build -t $(IMAGE_NAME):$(TAG) .

run:
	docker run -d --name $(IMAGE_NAME) -p 9876:9876 $(IMAGE_NAME):$(TAG)

stop:
	docker stop $(IMAGE_NAME) && docker rm $(IMAGE_NAME)

clean:
	docker rmi $(IMAGE_NAME):$(TAG)
