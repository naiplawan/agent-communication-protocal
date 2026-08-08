FROM python:3.12-slim

WORKDIR /app

RUN pip install --no-cache-dir flask httpx pyyaml python-socketio aiohttp

COPY lib/ /app/lib/
COPY mock_agent.py /app/mock_agent.py

ENV FLASK_APP=mock_agent
ENV PYTHONUNBUFFERED=1

CMD ["python", "mock_agent.py"]
