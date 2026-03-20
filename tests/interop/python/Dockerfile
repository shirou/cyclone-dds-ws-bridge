FROM python:3.12-slim-bookworm

WORKDIR /app

COPY tests/interop/python/requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

COPY tests/interop/python/ /app/scripts/

# Fixtures directory at the path the scripts expect (../fixtures relative to scripts)
RUN mkdir -p /app/fixtures

ENV FIXTURE_DIR=/app/fixtures

CMD ["sh", "-c", "python3 /app/scripts/generate_fixtures.py && python3 /app/scripts/verify_go_fixtures.py"]
