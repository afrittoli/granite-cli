# Custom Dockerfiles for AI Model Providers

This directory contains custom Dockerfiles for AI model providers that don't
have official Docker images or need customization for testing purposes.

## Structure

Each provider gets its own subdirectory:

```
docker/
├── README.md           this file
├── llama-cpp/          llama.cpp server
│   └── Dockerfile
└── lmstudio/           LM Studio (if containerizable)
    └── Dockerfile
```

## Usage in docker-compose.providers.yml

Reference custom images with the `build:` directive:

```yaml
services:
  llama-cpp:
    build:
      context: docker/llama-cpp
      dockerfile: Dockerfile
    ports:
      - "8080:8080"
    healthcheck:
      test: ["CMD", "curl", "-f", "http://localhost:8080/health"]
```

## Images in CI

CI does not build these images in every run. The `E2E images` workflow
builds each one that `tests/e2e/services.txt` marks `always` or `changed`
and pushes it to the ghcr.io
mirror with a tag computed from the service's `build:` section and the
files in its build context, and the `e2e-local` job pulls it from there.
Changing any file in the directory gives a new tag, so the image is rebuilt
once after the change reaches `main`. See "Provider images" in
`tests/e2e/README.md`.

Add `LABEL org.opencontainers.image.source=https://github.com/ibm-granite-community/granite-cli`
to a Dockerfile so that ghcr.io links its package to the repository.

## Building locally

```sh
# Build a custom image
docker build -t gcli-test-llama-cpp tests/e2e/docker/llama-cpp/

# Use it in compose
docker compose -f tests/e2e/docker-compose.providers.yml up -d llama-cpp
```

## Example Dockerfile

```dockerfile
FROM ubuntu:22.04

RUN apt-get update && apt-get install -y \
    curl \
    build-essential \
    && rm -rf /var/lib/apt/lists/*

# Download and build llama.cpp
RUN curl -L https://github.com/ggerganov/llama.cpp/archive/refs/tags/b1234.tar.gz | tar xz \
    && cd llama.cpp-* \
    && make server

EXPOSE 8080
CMD ["./llama.cpp-*/server", "-m", "/models/model.gguf", "--port", "8080"]
```
