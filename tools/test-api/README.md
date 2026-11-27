# Fastman Test API

Small Bun + Express container for testing Fastman chains (real `HttpOnly` cookies, cookie isolation, Bearer/Basic auth, CRUD lifecycles, cleanup steps, latency/status assertions).

## Run with Docker

```sh
docker compose -f tools/test-api/docker-compose.yml up -d --build
```

Or directly with Bun:

```sh
cd tools/test-api && bun install && bun run start
```

Server listens on `http://127.0.0.1:4010`.

## Import into Fastman

Click **Import** in Fastman and import `tools/test-api/fastman-collection.json`.

## Endpoints

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/health` | Health + session/item counts |
| `POST` | `/auth/login` | Sets `fastman_sid` cookie (`HttpOnly; SameSite=Lax`) + returns `/auth/access_token` and `X-Auth-Token` |
| `GET` | `/auth/me` | Verifies cookie and/or Bearer token, reports `auth_via`, `has_cookie`, `has_bearer` |
| `GET` | `/auth/cookie-only` | Requires `fastman_sid` cookie (401 when node has `Isolated` cookies enabled) |
| `GET` | `/auth/basic` | Verifies Basic auth (`demo:secret`) |
| `POST` | `/auth/logout` | Clears session and expires cookie (`204`) |
| `POST` | `/items` | Creates item (`201`, returns `/id`, `/slug`, `/version`) |
| `GET` | `/items` | Lists items |
| `GET` | `/items/:id` | Reads item (`200` or `404`) |
| `PATCH` | `/items/:id` | Updates item and increments `version` (`200` or `404`) |
| `DELETE` | `/items/:id` | Deletes item (`204` or `404`) |
| `GET` | `/delay/:ms` | Delays up to 5000 ms (latency/cancel tests) |
| `GET` | `/status/:code` | Returns chosen HTTP status (failure/cleanup tests) |
| `POST` | `/reset` | Clears all in-memory sessions and items (`204`) |
