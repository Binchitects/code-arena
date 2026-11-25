#!/bin/sh
# Runs a command in the build image, with this checkout at /src, as the current user.
# Proxy variables are passed on when set (crates.io may need one); local addresses never go through it.
set -eu
exec docker run --rm -u "$(id -u):$(id -g)" \
  ${HTTPS_PROXY:+-e HTTPS_PROXY="$HTTPS_PROXY"} ${HTTP_PROXY:+-e HTTP_PROXY="$HTTP_PROXY"} \
  -e NO_PROXY=127.0.0.1,localhost,::1 \
  -v "$(pwd)":/src -w /src code-arena-builder "$@"
