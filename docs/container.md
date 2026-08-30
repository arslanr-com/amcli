# Running `amcli web` in a container

The `Dockerfile` at the repository root builds a demo deployment: one stage
compiles the binary and, with it, generates the model from
`deploy/demo/meridian.jsonl`; the runtime stage carries only those two files.
Nothing is downloaded at run time and there is no secret anywhere in the image
— the model is fictional and the environment below is all configuration.

```sh
docker build -t amcli-demo .
docker run --rm -p 8080:3000 -e AMCLI_WEB_ALLOW_HOST=localhost amcli-demo
```

**The internal port is 3000** (`EXPOSE 3000`), and the process runs as uid
10001, non-root, with the model owned by root and world-readable: the viewer
serves GET and nothing else, so it never needs to write what it is showing.

## The environment

| Variable | Default in the image | What it is |
|---|---|---|
| `AMCLI_MODEL` | `/app/model/demo.archimate` | The model to serve. |
| `AMCLI_WEB_BIND` | `0.0.0.0` | The interface. Must be the wildcard for anything outside the container to reach it; `amcli web` binds loopback otherwise. |
| `AMCLI_WEB_PORT` | `3000` | The port inside the container. |
| `AMCLI_WEB_ALLOW_HOST` | `amcli.arslanr.com` | Comma-separated `Host` headers accepted besides loopback. |

None of these is a secret, and none needs to be. There is no variable that
takes a credential, because there is nothing to authenticate to.

`AMCLI_WEB_ALLOW_HOST` is the one to get right. Binding the wildcard is what
makes the port reachable; naming the host is what keeps the DNS-rebinding
defence that loopback-only binding used to provide on its own. A request whose
`Host` is neither loopback nor on that list is answered 403 — so behind a
reverse proxy, the list has to name whatever the proxy forwards, which is the
domain the reader typed. Set it to the deployment's domain; `*` accepts any
host and gives that defence up.

## Health

`HEALTHCHECK` polls `/api/status`, which answers 200 only once the model has
been parsed and is being served — a readiness signal rather than an open
socket. It asks over loopback, which is allowed whatever `AMCLI_WEB_ALLOW_HOST`
says, so the check keeps working when the domain changes.

## Behind Coolify

Point the proxy at container port 3000 and set `AMCLI_WEB_ALLOW_HOST` to the
domain it terminates. TLS is the proxy's job; the viewer speaks plain HTTP/1.1
and has no business holding a certificate.
