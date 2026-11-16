#!/bin/bash
# Links Windows programs with MinGW, translating the MSVC-style flags dx passes.
args=()
for a in "$@"; do
  case "$a" in
    /SUBSYSTEM:WINDOWS) args+=("-mwindows") ;;
    /ENTRY:*) ;;
    *) args+=("$a") ;;
  esac
done
exec x86_64-w64-mingw32-gcc "${args[@]}"
