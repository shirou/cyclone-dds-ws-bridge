FROM python:3.12-slim-bookworm

WORKDIR /app

COPY tests/e2e/python/requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

COPY tests/e2e/python/ /app/

CMD ["python3", "/app/e2e_test.py"]
