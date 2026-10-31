# Spec Delta

## ADDED Requirements

### Requirement: Video poster frames once a tool is available
A video whose placeholder waits for a poster-frame tool SHALL count as waiting whenever a tool is available now, and Fill in previews and the next rescan of its source SHALL run the tool on it once per run, for one copy of each content, without re-reading its content or changing its identity. A failed extraction SHALL be recorded as such and SHALL NOT be retried until the file or the video poster generation changes, or the user asks.

#### Scenario: Fill in gives old placeholder videos their poster frames
- **WHEN** a catalog with 4 videos waiting for a poster-frame tool and 10 photos runs Fill in previews with a working tool now available, through a byte-counting file layer
- **THEN** the tool runs once per video, the 4 get poster-frame thumbnails with their identities unchanged, and no file is opened by PhotoSquirrel

#### Scenario: Rescan fills them in once
- **WHEN** the unchanged source is rescanned with the tool available, and then rescanned again
- **THEN** the first rescan runs the tool once per waiting video and the second runs it not at all

#### Scenario: Nothing waits without a tool
- **WHEN** no poster-frame tool is available
- **THEN** Fill in previews offers no video and a rescan runs no tool

#### Scenario: Failed extraction is not retried forever
- **WHEN** the tool fails on one waiting video, and Fill in previews and a rescan run again
- **THEN** the video records "poster frame failed" with the error and the tool is not run on it again

#### Scenario: Explicit retry of failed posters
- **WHEN** the user runs Fill in previews for the reason "poster frame failed"
- **THEN** the tool runs once on each such video

#### Scenario: Offline videos are skipped
- **WHEN** 2 of the waiting videos are on an offline source
- **THEN** the job leaves those 2 waiting, reports 2 skipped because offline, and fills in the rest

#### Scenario: Cancel stops a running extraction
- **WHEN** the user cancels Fill in previews while the tool is running on a video
- **THEN** the tool process is ended, that video and the rest still wait, and the next run fills them in

### Requirement: Poster-frame tool calls are bounded
A run SHALL make at most two poster-frame tool calls at once, one on a rotational or network source, whether for waiting videos or for the analysis of new ones. A failure that says nothing about the file (the source went offline or the file changed during the call, or the call timed out) SHALL leave the video waiting.

#### Scenario: Temporary failure leaves the video waiting
- **WHEN** the tool times out on the waiting videos, or their source goes offline during the call
- **THEN** the videos keep waiting for a poster-frame tool, no "poster frame failed" is recorded, and the next run tries again

#### Scenario: Tool calls are limited during analysis too
- **WHEN** a first scan analyses 6 new videos with 6 CPU workers on a solid-state source, and again on a network source
- **THEN** at most 2 tool calls run at once on the solid-state source and 1 on the network source
