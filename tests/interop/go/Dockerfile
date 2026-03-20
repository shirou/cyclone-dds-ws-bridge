FROM golang:1.26-bookworm

WORKDIR /app/tests/interop/go

COPY tests/interop/go/go.mod tests/interop/go/go.sum ./
RUN go mod download

COPY tests/interop/go/ ./

# Create fixtures directory at the path the test expects (../fixtures)
RUN mkdir -p /app/tests/interop/fixtures

CMD ["go", "test", "-v", "./..."]
