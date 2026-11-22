#!/bin/sh

# Run from the checkout root. Take a fresh snapshot for each benchmark build.
unset RUST_GLANCER_BUILD_SOURCE
if [ -e .git ] && revision=$(git rev-parse --short=12 HEAD 2>/dev/null) &&
    status=$(git --no-optional-locks status --porcelain --untracked-files=normal 2>/dev/null)
then
    state=clean
    if [ -n "$status" ]; then
        state=dirty
    fi

    RUST_GLANCER_BUILD_SOURCE="$revision, $state"
    if branch=$(git symbolic-ref --quiet --short HEAD 2>/dev/null) && [ "$branch" != main ]; then
        RUST_GLANCER_BUILD_SOURCE="$RUST_GLANCER_BUILD_SOURCE, $branch"
    fi
    export RUST_GLANCER_BUILD_SOURCE
fi

exec "$@"
