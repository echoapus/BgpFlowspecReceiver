"""Async event bus — session components emit here, SSE clients subscribe."""

import asyncio
from collections import deque
from datetime import datetime, timezone
from typing import Optional


class EventBus:
    def __init__(self, maxlen: int = 2000):
        self._history: deque = deque(maxlen=maxlen)
        self._queues: list[asyncio.Queue] = []

    def emit(
        self,
        event_type: str,
        level: str,
        message: str,
        **extra,
    ) -> dict:
        event = {
            "ts":      datetime.now(timezone.utc).isoformat(),
            "type":    event_type,
            "level":   level,
            "message": message,
            **extra,
        }
        self._history.append(event)
        for q in list(self._queues):
            try:
                q.put_nowait(event)
            except asyncio.QueueFull:
                pass
        return event

    def subscribe(self) -> asyncio.Queue:
        q: asyncio.Queue = asyncio.Queue(maxsize=500)
        self._queues.append(q)
        return q

    def unsubscribe(self, q: asyncio.Queue) -> None:
        try:
            self._queues.remove(q)
        except ValueError:
            pass

    def history(self) -> list[dict]:
        return list(self._history)

    def clear(self) -> None:
        self._history.clear()
