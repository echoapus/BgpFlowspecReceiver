FROM python:3.11-slim

WORKDIR /app

COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

COPY . .
RUN pip install --no-cache-dir -e .

EXPOSE 8080

ENTRYPOINT ["bgpx"]
CMD ["--host", "0.0.0.0", "--port", "8080"]
