FROM python:3.12-slim

WORKDIR /app
COPY tests/client/requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

COPY tests/client/ .

CMD ["pytest", "-v", "--tb=short"]
