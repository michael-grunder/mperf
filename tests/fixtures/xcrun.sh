#!/bin/sh
if [ "$1" = "--find" ]; then echo mock-xctrace; exit 0; fi
shift
mode=$1
shift
if [ "$mode" = record ]; then
  while [ "$#" -gt 0 ]; do
    if [ "$1" = --output ]; then shift; output=$1; fi
    shift
  done
  mkdir -p "$output"
  cp "$MPERF_FIXTURE" "$output/samples.xml"
  if [ "$MPERF_WAIT" = 1 ]; then
    trap 'exit 0' INT
    printf ready > "$MPERF_READY"
    while :; do sleep 0.05; done
  fi
  if [ "$MPERF_FAIL" = 1 ]; then echo 'mock recorder failed' >&2; exit 42; fi
  exit 0
fi
if [ "$mode" = export ]; then
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --input) shift; input=$1;;
      --toc) toc=1;;
      --xpath) shift; printf '%s' "$1" > "$MPERF_QUERY_LOG";;
    esac
    shift
  done
  if [ "$toc" = 1 ]; then
    printf '%s' '<trace-toc><run number="1"><data><table schema="time-profile"/></data></run></trace-toc>'
  else
    cat "$input/samples.xml"
  fi
  exit 0
fi
exit 1
