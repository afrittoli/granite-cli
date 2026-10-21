#!/usr/bin/env bash
# Get the provider images that docker-compose.providers.yml uses from a
# mirror registry, such as ghcr.io, and keep that mirror up to date.
#
# Usage:
#   tests/e2e/images.sh pull SERVICE...    make each service's image
#                                          available locally
#   tests/e2e/images.sh push SERVICE...    copy or build each service's image
#                                          into the mirror
#   tests/e2e/images.sh ref SERVICE        print the image compose runs
#   tests/e2e/images.sh mirror-ref SERVICE print the image in the mirror
#
# pull takes the image from the mirror when the mirror has it, and tags it
# with the name compose uses, so `docker compose up` runs it without
# pulling or building. Otherwise the image is new or changed: pull pulls it
# from its own registry, or builds it for a service with a `build:` section.
# It reports where each image came from, also in the GitHub Actions job
# summary.
#
# The mirror holds SERVICE's image as $E2E_IMAGE_MIRROR/granite-cli-e2e-SERVICE:
#   - for a service that names an image, with that image's tag, for example
#     ollama/ollama:0.40.0 -> granite-cli-e2e-ollama:0.40.0;
#   - for a service with a `build:` section, with ctx-<hash> as the tag,
#     where <hash> covers the build section and every file in the build
#     context, so any change to them gives a new tag.
#
# push copies an upstream image with all its platforms, registry to
# registry, and overwrites the tag, so a moving upstream tag is refreshed.
# For a built image it builds and pushes only when the tag is missing.
# push needs `docker login` to the mirror.
#
# Environment:
#   E2E_IMAGE_MIRROR    registry and namespace of the mirror, for example
#                       ghcr.io/ibm-granite-community. Unset: pull uses the
#                       images' own registries, and push fails.
#   E2E_IMAGE_SOURCE    repository URL that push records on copied images
#                       (org.opencontainers.image.source), which ghcr.io
#                       uses to link the package to the repository. A built
#                       image gets it from a label in its Dockerfile.
#   OLLAMA_VERSION and the other variables docker-compose.providers.yml
#   reads select the images, as for `docker compose up`.
#
# Needs docker with the compose and buildx plugins, git and jq.

set -euo pipefail

E2E_DIR=$(cd "$(dirname "$0")" && pwd)
COMPOSE_FILE="$E2E_DIR/docker-compose.providers.yml"

usage() {
    sed -n '2,43p' "$0" | sed 's/^# \{0,1\}//' >&2
    exit 2
}

# Prints the resolved compose configuration as JSON.
compose_json() {
    docker compose -f "$COMPOSE_FILE" config --format json
}

# Prints one service's configuration as JSON; fails for an unknown service.
service_json() {
    compose_json | jq -e --arg s "$1" '.services[$s] // error("no service \($s)")'
}

# Prints the image compose runs for SERVICE: its `image:`, or for a service
# with only a `build:` section, the <project>-<service> name compose builds.
local_ref() {
    compose_json | jq -er --arg s "$1" '
        if .services[$s] == null then error("no service \($s)")
        else .services[$s].image // "\(.name)-\($s)" end'
}

# Prints the build context directory of SERVICE, or nothing when SERVICE
# names an image without building it.
build_context() {
    service_json "$1" | jq -r '.build.context // empty'
}

# Prints a hash of SERVICE's build section and of the content of every
# file in its build context that git does not ignore. The context's own
# path is left out: compose resolves it to an absolute path, which differs
# between checkouts.
context_hash() {
    local ctx=$1 service=$2
    {
        service_json "$service" | jq -cS '.build | del(.context)'
        (cd "$ctx" && git ls-files --cached --others --exclude-standard . | LC_ALL=C sort |
            while IFS= read -r f; do
                [ -f "$f" ] && printf '%s %s\n' "$(git hash-object -- "$f")" "$f"
            done)
    } | git hash-object --stdin | cut -c1-16
}

mirror_ref() {
    local service=$1 mirror ctx ref tag
    mirror=$(printf '%s' "${E2E_IMAGE_MIRROR:?set E2E_IMAGE_MIRROR to the mirror, for example ghcr.io/OWNER}" |
        tr '[:upper:]' '[:lower:]')
    ctx=$(build_context "$service")
    if [ -n "$ctx" ]; then
        tag="ctx-$(context_hash "$ctx" "$service")"
    else
        ref=$(local_ref "$service")
        ref=${ref%@*}
        case "${ref##*/}" in
            *:*) tag=${ref##*:} ;;
            *) tag=latest ;;
        esac
    fi
    printf '%s/granite-cli-e2e-%s:%s\n' "${mirror%/}" "$service" "$tag"
}

# Prints a line, and adds it to the GitHub Actions job summary when there
# is one.
report() {
    echo "$1"
    if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
        printf -- '- %s\n' "$1" >>"$GITHUB_STEP_SUMMARY"
    fi
}

# An image the mirror does not have is new or changed, for example in a pull
# request that changes its version or its Dockerfile: pull builds or pulls
# it, so that the tests that follow run against it.
pull() {
    local service=$1 image mirrored note=""
    image=$(local_ref "$service")
    if [ -n "${E2E_IMAGE_MIRROR:-}" ]; then
        mirrored=$(mirror_ref "$service")
        if docker pull --quiet "$mirrored" >/dev/null 2>&1; then
            docker tag "$mirrored" "$image"
            report "$service: $image from the mirror ($mirrored)"
            return
        fi
        note=", not in the mirror ($mirrored): new or changed"
    fi
    if [ -n "$(build_context "$service")" ]; then
        docker compose -f "$COMPOSE_FILE" build "$service"
        report "$service: $image built$note"
    else
        docker pull --quiet "$image" >/dev/null
        report "$service: $image pulled from its registry$note"
    fi
}

push() {
    local service=$1 image mirrored ctx
    image=$(local_ref "$service")
    mirrored=$(mirror_ref "$service")
    ctx=$(build_context "$service")
    if [ -z "$ctx" ]; then
        set -- docker buildx imagetools create --tag "$mirrored"
        if [ -n "${E2E_IMAGE_SOURCE:-}" ]; then
            set -- "$@" --annotation "index:org.opencontainers.image.source=$E2E_IMAGE_SOURCE"
        fi
        "$@" "$image"
        echo "$service: copied $image to $mirrored"
    elif docker buildx imagetools inspect "$mirrored" >/dev/null 2>&1; then
        echo "$service: $mirrored is already in the mirror"
    else
        docker compose -f "$COMPOSE_FILE" build "$service"
        docker tag "$image" "$mirrored"
        docker push --quiet "$mirrored" >/dev/null
        echo "$service: built and pushed $mirrored"
    fi
}

[ $# -ge 2 ] || usage
cmd=$1
shift
case "$cmd" in
    pull | push)
        for service in "$@"; do "$cmd" "$service"; done
        ;;
    ref)
        [ $# -eq 1 ] || usage
        local_ref "$1"
        ;;
    mirror-ref)
        [ $# -eq 1 ] || usage
        mirror_ref "$1"
        ;;
    *) usage ;;
esac
