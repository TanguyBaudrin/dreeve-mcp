# dreeve-mcp

An [MCP](https://modelcontextprotocol.io) server exposing a self-hosted
[Dreeve](https://dreeve.app) instance over **Streamable HTTP**, written in Rust
with the official [rust-sdk](https://github.com/modelcontextprotocol/rust-sdk).

The 15 tools map 1:1 onto the [Dreeve HTTP API v1](https://docs.dreeve.app/api/).

## Tools

| Group | Tool | API operation |
|---|---|---|
| Status | `check_status` | `GET /api/v1/status` |
| Activities | `list_activities` | `GET /api/v1/activities` |
| | `get_activity` | `GET /api/v1/activities/{id}` |
| | `update_activity` | `PATCH /api/v1/activities/{id}` |
| | `delete_activity` | `DELETE /api/v1/activities/{id}` |
| | `download_activity_gpx` | `GET /api/v1/activities/{id}/gpx` |
| Images | `add_activity_image` | `POST /api/v1/activities/{id}/images` |
| | `remove_activity_image` | `DELETE /api/v1/activities/{id}/images/{imageId}` |
| Imports | `upload_activity_file` | `POST /api/v1/activity/upload` |
| | `list_file_imports` | `GET /api/v1/file-imports` |
| Gear | `list_gear` | `GET /api/v1/gear` |
| | `list_maintenance_tasks` | `GET /api/v1/gear/maintenance-tasks` |
| Weight | `list_weights` | `GET /api/v1/athlete/weights` |
| | `record_weight` | `POST /api/v1/athlete/weights` |
| | `delete_weight` | `DELETE /api/v1/athlete/weights/{on}` |

## Configuration

| Variable | Required | Default | Description |
|---|---|---|---|
| `DREEVE_API_URL` | | `http://localhost:8080` | Base URL (`APP_URL`) of your Dreeve instance |
| `MCP_BIND_ADDR` | | `0.0.0.0:8080` | Listen address of the MCP HTTP endpoint |
| `MCP_ALLOWED_HOSTS` | | loopback only | Extra `Host` authorities accepted by the endpoint (`host` or `host:port`, comma-separated), e.g. `strava.baudrin.fr,10.0.5.190:8081`. `*` accepts any host. `localhost`, `127.0.0.1` and `::1` always stay |
| `RUST_LOG` | | `info` | Log filter (`tracing-subscriber` `EnvFilter`) |
| `SSL_CERT_FILE` | | `/etc/ssl/certs/ca-certificates.crt` | CA bundle used for TLS to Dreeve |

The server holds **no API key**: each MCP client sends its own key on every
request as an `Authorization: Bearer drv_…` header, which is forwarded
verbatim to the Dreeve API. Generate a key in Dreeve under
*Settings → Security → API access*. Requests to `POST /mcp` without a
`Bearer` token are rejected with `401 Unauthorized`.

## Running

### Docker

```sh
docker run --rm -p 8080:8080 \
  -e DREEVE_API_URL=https://dreeve.example.com \
  ghcr.io/tanguybaudrin/dreeve-mcp:latest
```

The MCP endpoint is served at `http://localhost:8080/mcp`.

The final image is built `FROM scratch` around a statically linked musl
binary: a few MB, no OS packages, non-root (`65532`).

> **File uploads** (`upload_activity_file`, `add_activity_image`) take a file
> *path on the machine running dreeve-mcp*. Mount the directory containing your
> `.fit`/`.tcx`/`.gpx` files and pass the path as seen inside the container:
> ```sh
> docker run --rm -p 8080:8080 \
>   -v "$HOME/activities:/data:ro" \
>   ghcr.io/tanguybaudrin/dreeve-mcp:latest
> # → upload_activity_file(path = "/data/morning-ride.fit")
> ```

### Claude Code

```sh
claude mcp add --transport http \
  --header "Authorization: Bearer drv_xxx" \
  dreeve http://localhost:8080/mcp
```

### Any MCP client

Send your Dreeve API key as an `Authorization` header on every request:

```json
{
  "mcpServers": {
    "dreeve": {
      "type": "streamable-http",
      "url": "http://localhost:8080/mcp",
      "headers": {
        "Authorization": "Bearer drv_xxx"
      }
    }
  }
}
```

### From source

```sh
export DREEVE_API_URL=http://localhost:8080
cargo run --release
```

## Development

A [devcontainer](.devcontainer/devcontainer.json) is provided (Rust toolchain,
`gh`, and Docker-outside-of-Docker for building images from inside the
container). Port `8080` is forwarded automatically.

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
```

### Docker image build

Multi-stage: `rust:1-alpine` (static musl build) → `scratch`.

```sh
docker build -t dreeve-mcp .
docker run --rm -p 8080:8080 dreeve-mcp
```

## Continuous integration

- **CI** (`.github/workflows/ci.yml`) — `rustfmt`, `clippy -D warnings`, tests
  and a release build on every push/PR to `main`.
- **Docker** (`.github/workflows/docker.yml`) — on every push to `main`, every
  `v*.*.*` tag, or manually: builds `linux/amd64` on a native amd64 runner and
  `linux/arm64` on a native arm64 runner, pushes per-arch tags to GHCR, then
  merges them into multi-arch manifests (`latest`, semver, branch and SHA tags).

## License

[MIT](LICENSE)
