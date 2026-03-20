FROM golang:1.26-bookworm

WORKDIR /app

# Copy ddswsclient (dependency via replace directive)
COPY ddswsclient/ ddswsclient/

# Copy e2e test module
COPY tests/e2e/go/ tests/e2e/go/

WORKDIR /app/tests/e2e/go
RUN go mod download

CMD ["go", "test", "-v", "-timeout", "120s", "./..."]
