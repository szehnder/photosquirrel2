# 0009. Threaded pipeline with per-volume I/O lanes

- Status: accepted
- Date: 2026-10-17
- Deciders: maintainers

## Context
Sources are a mix of SSDs, spinning USB drives and NAS shares. Spinning disks collapse under random concurrent reads; SSDs and NAS need concurrency to reach throughput. Decoding and hashing are CPU-bound.

## Decision
The scan engine uses plain OS threads, bounded `crossbeam-channel` queues (back-pressure) and `rayon` for CPU work - no async runtime. Each source gets an **I/O lane**: a pool of reader threads whose size adapts between 1 and 16 using measured throughput (start: 2 for rotational/unknown, 8 for SSD; grow while MB/s improves by >10%, shrink when latency rises). Within a lane, files are read in directory order to keep spinning disks sequential. Readers hand whole-file buffers to a shared CPU pool sized to the cores minus one for the UI. tokio is used only for network I/O (LLM providers).

## Alternatives considered
- **tokio for file I/O** - tokio fs is a thread pool in disguise with extra overhead.
- **Fixed global concurrency** - wrong for every device class at once.

## Consequences
- Memory is bounded: at most `lanes x 2` file buffers in flight plus a 512 MB global cap.

## Revisit when
io_uring / IOCP become worth it for NAS throughput.
