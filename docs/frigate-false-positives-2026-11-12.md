# Frigate `person` false positives — 158,363 events, two GPUs, one vision model, and the one lever that measures

**What this is:** the 2026-11-11/12 investigation into why Frigate on `services1` reports
people who are not there. It covers the shape of the false-positive population read out of
the live event database, the detector-capacity question (Coral vs the iGPU vs OpenVINO), the
architecture of GPU offload in Frigate 0.18.2, why the `services3` GTX 1080 Ti cannot be
Frigate's detector whatever it is pointed at, a two-round vision-LLM verifier experiment on
that card, and — the part worth acting on — the one lever that measures: a box-size floor
that drops ~75 % of the never-moving tracks while costing ~7 % of the moving ones.

**Nothing in this document has been applied.** `/srv/frigate/config/config.yaml` is
unchanged by it, `podman-frigate` was never restarted, and the proposed change is in §8
awaiting a decision. The `n=` figures come from a copy of the event DB; the way to make
another one without corrupting anything is `sudo sqlite3 /srv/frigate/config/frigate.db
".backup /tmp/fdb.db"` (the `.db-wal` lesson in `docs/follow-ups.md` applies — a bare `cp`
of a running WAL database is not a backup).

## 1. How the cohorts are defined, and the one thing that is not what it looks like

Everything below compares two populations:

- **NEVER-MOVED** — a `person` event whose `path_data` spans less than `0.02` of the frame
  width over its whole life. At 4K that is under ~77 px of travel, and the tracks are long:
  `> 300 s`.
- **MOVING** — `path_data` span ≥ `0.08`, duration 10–240 s. Real people, by construction.

Three facts about the data that cost time to find:

- `box` in `event.data` is normalised `[x1, y1, w, h]`, and it is normalised to the **detect**
  frame, not the snapshot. `social_space`'s snapshots are 2304×1296 and its detect frame is
  1280×720; a box height computed against the wrong one is 1.8× out.
- `average_estimated_speed` is **0.0 for every event in the database**, not because the
  feature is broken: `track/tracked_object.py` only computes it inside a zone that has
  `distances` set (`:242`, and the `speed_threshold` gate at `:250`). There are no zones.
  A speed filter is therefore an available lever, not a dead one.
- `smart_motion` **does not exist in 0.18.2** (`grep -rln smart_motion /opt/frigate/frigate`
  is empty). It is not in this build and must not be written into the config.

## 2. What the false-positive population actually is

365 days at the copy: **158,363 events**. Per camera (30-day window, event count, and the
never-moved cohort's share of events and of *event-hours*):

| camera | events | /day | never-moved | of hours | longest |
| --- | --- | --- | --- | --- | --- |
| main_space | 7,877 | 262.6 | **2,031 (25.8 %)** | **24.2 %** | 701 min |
| workshop | 3,093 | 103.1 | 261 (8.4 %) | 7.4 % | 3 min |
| woodwork | 1,813 | 60.4 | 114 (6.3 %) | 1.8 % | 18 min |
| cam8 | 841 | 28.0 | 123 (14.6 %) | 9.0 % | 20 min |
| rack_corridor | 560 | 18.7 | 126 (22.5 %) | 3.4 % | 4 min |
| social_space | 425 | 14.2 | 48 (11.3 %) | **25.8 %** | 238 min |
| cnc | 124 | 4.1 | 12 (9.7 %) | 3.0 % | 1 min |

Whole network, 30 days: 14,739 events (491.3/day), never-moved 2,715 = **18.4 % of events
and 20.5 % of event-hours**. main_space alone holds 99,060 of the 158,363 events (271.7/day
over the year).

The rest of the anatomy, all from the same copy:

- **The tracks do not end.** main_space durations: median 46 s, p90 589 s, max **11.7 h**;
  29.8 % exceed 120 s and 9.8 % exceed 10 minutes.
- **The detector's confidence is capped far below 1.** The observed maximum `person` score
  across the whole DB is **0.844**, with main_space at median 0.80 / p10 0.61 — while the
  per-camera filter is `min_score: 0.5, threshold: 0.7`. A third of main_space's events sit
  below 0.70.
- **Detection runs 1.5–1.7× the requested rate.** The eight cameras ask for 24.8 detect-fps and
  the pipeline runs ~41.6 region-inferences/s — **1.68× overall, 1.46× on `main_space`** (7.3
  against `fps: 5`): motion fires on essentially every frame, so the weak model gets half again
  to twice as many chances to be wrong. `skipped` is 0 — this is not a load-shedding artefact.
- **`stationary.max_frames` is unset**, which means "never expire" (§7).

## 3. Detector capacity is not the constraint, and a bigger model is not free

| | per inference | note |
| --- | --- | --- |
| Coral TPU (`type: edgetpu, device: pci`) | **8.87 ms** → ~113 inf/s | measured demand ~41 inf/s ⇒ **~37 % utilised** |
| OpenVINO `device: CPU`, same 300² ssdlite | 12.9–14.6 ms | slower than the Coral |
| OpenVINO `device: GPU` (the iGPU), same model | **8.2–8.3 ms** → ~121 inf/s | matches/beat the Coral for ~70 % less CPU |
| 640² model on that iGPU | 34.8 ms → ~28 inf/s | **under** the ~41 inf/s of demand |

So: the Coral is not the bottleneck, and no FP reduction follows from more throughput. But
the last row is the honest reason a discrete GPU enters the conversation at all — **a
materially better primary model does not fit on this box's iGPU.** Meanwhile the box is
CPU- and shared-memory-bound for other reasons: load average 6.5 on 4 cores, `dec` 30.8 % /
compute 0.1 % on the iGPU, `/dev/shm` 968 MB of 1152 MB (41.8 MB frames), and
`social_space` reported `stalls_last_hour: 3`.

## 4. Pascal: the 1080 Ti has the horsepower and Frigate cannot address it

`services3` (bench box, GTX 1080 Ti / GP102) measured on 2026-11-12: `compute_cap 6.1`,
driver `580.178.04` (`legacy_580`), `/dev/nvidia*` + `/dev/nvidia-uvm*` present,
`/run/opengl-driver/lib/libcuda.so.1` present, Vulkan 1.4.312 with the card as device 0,
11,264 MiB of which LFM3 holds 3,026 MiB, **no iGPU at all** (HD 530 disabled in firmware,
so no QSV), no swap, and a USB Wi-Fi uplink on VLAN 226 whose RTT from `services1` is
min 1.788 / avg 28.679 / **max 186.721** / mdev 59.912 ms.

| Layer | Status |
| --- | --- |
| Driver | Works — `legacy_580` is the last branch with Pascal |
| CUDA **driver** library | Present, so *prebuilt* CUDA ≤ 12.x userland can still run |
| CUDA **toolkit** | Dead: CUDA 13's `nvcc` emits no sm_61 (`machines/services3/nvidia.nix`), and nixpkgs' `onnxruntime` builds with `cudaPackages.cuda_nvcc`, so no sm_61 cubin is producible on this network |
| TensorRT | Frigate's `-tensorrt` image ships TRT ≥ 10, whose device support starts above Pascal |
| FP16 / INT8 | GP102 has no FP16 *arithmetic* (1:64) and no INT8 instructions ⇒ **FP32 only** |
| Vulkan | Alive and in production on this very card |

**The structural finding:** Frigate 0.18.2's detector plugins are
`axengine cpu_tfl deepstack edgetpu_tfl hailo8l memryx onnx openvino rknn synaptics
teflon_tfl tensorrt zmq_ipc`, auto-imported by `detectors/detector_types.py` from
`DetectionApi.__subclasses__()`. Cross-checked against sm_61 that set contains **zero
usable detectors** — OpenVINO is Intel-only, Coral/Teflon are Coral, TensorRT wants Turing
or newer, and the ONNX CUDA EP cannot be built for Pascal here. That is true whether the
card sits in `services3`, in `services1`, or in a box on the moon: **Pascal can only ever be
a sidecar to Frigate, never its detector.**

The only two doors are both in that plugin list:

- **`zmq`** (`plugins/zmq_ipc.py`) — *"offloads inference to an external process"*. REQ/REP,
  request `[{"shape":[…],"dtype":…}, raw_tensor]`, response `[20,6] float32`; the endpoint
  goes straight to `socket.connect(...)`, so **`tcp://host:port` is valid**; it pushes your
  model file on first connect (`model_request` / `model_data`, 30 s). `request_timeout_ms`
  defaults to **200**, and on any timeout **it returns a zero array** — a dead sidecar looks
  to Frigate like an empty room.
- **`deepstack`** — its own docstring reads *"Not recommended"*; JPEG over HTTP,
  `api_timeout` 0.1 s, same silent-zero failure. Its GPU server stack is Turing-or-newer
  anyway.

The card is *not* slow. `llama-bench` on it, same backend any CNN would use (Vulkan, all
layers resident, build `d812350`):

```
lfm3 1.7B Q4_K-M  params 1.73 B  backend Vulkan   pp128   1467.05 ± 18.18 t/s
                                                  pp512   1647.28 ±  3.46 t/s
                                                  pp2048  1618.73 ±  0.75 t/s
```

`2 × 1.73e9 × 1,618 ≈ **5.6 TFLOP/s** of sustained batched matmul — ~49 % of its 11.3
TFLOPS FP32 peak. Against ncnn's published Vulkan table for a desktop RTX 2080
(`mobilenet_v2 2.00 ms`, `mobilenet_ssd 3.09`, `resnet50 4.49`, `vgg16 7.80`) halved for the
FP32 handicap, a YOLOX-s/YOLOv8s at 640² lands at **6–10 ms**, i.e. 100–150 inf/s against
the ~41 this network needs. **Horsepower was never the answer; the missing piece was a
server, and it would be a server we own, on a driver branch with an expiry date.** The
card's real niche on this network is CUDA-free inference — which is exactly the job it has.

## 5. Things Frigate 0.18.2 already has that nobody was using

- **Custom object classification** (`classification.custom.<name>`) — train a MobileNetV2 on
  your own detections in the UI; docs: "lightweight and run very fast on CPU", requires
  AVX+AVX2 (`services1` has `avx2` on all 4 cores). Results are assigned as `sub_label` or
  an `attribute` after **≥3 attempts with 60 % consensus** above `threshold` (default 0.8).
  It does **not** reject a detection. The docs' own training advice is to mix false positives
  deliberately into the `none` class, and the example grid is built from your stored events —
  i.e. it is the built-in, zero-hardware version of "second opinion on this crop".
- **`review.genai` writes descriptions and sub-labels only.** Severity comes from
  `review/maintainer.py`, computed from config — so GenAI cannot demote an alert. Verified by
  reading `data_processing/post/review_descriptions.py`, which *reads* `severity`.
- **Out-of-band correction, and its trap:** `PUT /api/events/{id}/false_positive` is **not a
  local flag** — it is the Frigate+ submission endpoint (400 `PLUS_API_KEY environment
  variable is not set`, and it uploads the crop off-box). What *is* local: `POST
  /api/events/{id}/sub_label`, `PUT /api/events/{id}/end`, and the review routes
  `set_multiple_reviewed` / `set_not_reviewed`. All require the `admin` role.
- **Retention kills retrospective verification.** Two event snapshots that answered `200` on
  2026-11-11 answered `404` on 2026-11-12. A verifier must run at event time; "re-process
  last month" is not an available design.

## 6. The vision-LLM verifier: two rounds, and what it proved

`Qwen2.5-VL-7B-Instruct` Q4_K_M + Q8_0 mmproj (5.6 GB) served by the **already-installed**
`llama-cpp-0.9.0` on `services3` — `bin/` contains `llama-mtmd-cli`, so multimodal is
compiled in and no inference code was written. It ran on `127.0.0.1:8099` **alongside**
production LFM3 at 9,445 / 11,264 MiB; the running service never had to be stopped.
Throughput: median **1.62 s/image**, vision prefill ~**214 tok/s** against the text prefill
rate of 1,618 (the vision tower sets the pace), and one 2193×2160 crop cost **17.9 s** —
input size must be capped.

Test sets were crops taken from the events' own boxes, out of the stored snapshots: round 1 =
7 never-moved + 12 moving; round 2 = 8 fresh never-moved + 15 fresh moving, with the crop
policy fixed (upscale capped at 2× instead of 6×, long side capped at 1024).

| | verdict-first prompt | description-first prompt |
| --- | --- | --- |
| R1 never-moved (7) | 7/7 PERSON | 4/7 PERSON |
| R1 moving (12) | 12/12 PERSON | 9/12 PERSON |
| R2 never-moved (8) | **6/8 NOT_PERSON** | **7/8 PERSON** |
| R2 moving (15) | 12/15 PERSON | 15/15 PERSON |

Round 2 is the **opposite** of round 1, so round 1's "ask it to describe first and it stops
flipping" was noise. It is not really looking either: across 46 calls the nouns it offered
were `a living human` ×27, `a photo of a person` ×9, `indistinct` ×7, `a mirror image` ×2,
`clothing on a hook` ×1 — and `a photo of a person` came up repeatedly on **moving people**,
with `verdict: PERSON` on the next line contradicting it. The two-frame temporal variant (51 h
and 56 h apart, fixed pixel window) kept the correct verdicts but answered
`DIFFERENT_OBJECTS` on static spots; that lever was dropped.

Two things came out of it.

1. **The usable signal was self-consistency, not the verdict.** Round 2's two prompt orders
   disagreed on **7 of 8 never-moved** crops and **3 of 15 moving** ones. "Both orders agree
   it is a person" ⇒ 12/13 = **92 % precision**, catching 80 % of real people while
   escalating 7/8 of the junk. That is a triage ranker (2 calls, ~3 s per event), not a
   filter — and it is the only thing in the whole experiment worth keeping.
2. **The premise "never-moved ⇒ false positive" is only partly true.** Given the crops, the
   operator confirmed two of the "PERSON" verdicts were *not* people — and the model's other
   answers for that cohort were "a person sitting on a bench" and "a living human" on an
   11.7-hour track. Some of those tracks are people who sit still, which is why §7's
   `stationary.max_frames` advice is now withdrawn and §8's filter is judged on geometry
   rather than on the proxy.

Why the line stops at 7B: the confirmed misses are **82 px** and **147 px** tall in the
detect frame. The models that would be dramatically better need 19–40 GB of VRAM and cannot
run on this card. The credible next step, if it is ever taken, is `Qwen3-VL` at 2–4 B —
the pinned `llama-cpp-upstream` (`7bcbc04`) already has `QWEN3VL` in `tools/mtmd/mtmd.cpp`,
and official GGUFs exist (`ggml-org/Qwen3-VL-2B-Instruct-GGUF`,
`Qwen/Qwen3-VL-4B-Instruct-GGUF`) — one download and the same script. Only worth doing
*after* §8, because the filter changes which crops survive to be judged.

## 7. The levers, ranked by measurement

### What works: a floor on box size

Box **height** separates the two populations almost cleanly (full DB, n = 2,181 never-moved
vs 66,941 moving):

| camera | never-moved height p10/p50/p90 | moving height p10/p50/p90 |
| --- | --- | --- |
| main_space | 75 / **92** / 213 px | 194 / **400** / 1124 px |
| woodwork | 43 / 49 / 140 | 184 / 435 / 555 |
| workshop | 44 / 227 / 282 | 1021 / 1436 / 1725 |

```
global, a height floor of:  <100 px -> drops 53.4% of never-moved,  0.9% of moving
                           <150 px ->                          66.5%,        3.9%
                           <200 px ->                          82.8%,       11.1%
                           <250 px ->                          94.9%,       19.8%
```

Frigate's `FilterConfig` has **no `min_height`** — the fields are `min_area`, `max_area`,
`min_ratio`, `max_ratio`, `threshold`, `min_score`, `mask`, `raw_mask` — so the floor has to
be expressed as an area, and `min_area` accepts a pixel count or a fraction of the frame.
Per-camera cost, measured on the 30-day cohorts (cut at each camera's never-moved p75):

| camera | `min_area` (px²) | as frame fraction | drops never-moved | costs moving | do it |
| --- | --- | --- | --- | --- | --- |
| **main_space** | 22,932 | `0.00276` | **74.9 %** | **6.9 %** | **yes** |
| **workshop** | 472,236 | `0.05693` | 74.7 % | 6.5 % | **yes** |
| social_space | 30,383 | `0.03297` | 72.9 % | 19.6 % | maybe, lower |
| cam8 | 15,624 | `0.00753` | 74.0 % | 22.6 % | no |
| woodwork | 42,460 | `0.02048` | 73.7 % | 50.8 % | no |
| rack_corridor | 16,206 | `0.01758` | 73.8 % | 49.9 % | no |

Apply it where the populations are genuinely different sizes — `main_space` and `workshop`,
which are also the two cameras carrying the most never-moved hours — and not where they
overlap. Both of the operator-confirmed misses fall under these floors (82 px and 147 px).

### What I recommended before and now do not

`stationary.max_frames` expires a track once
`motionless_count - stationary.threshold > max_frames` (`track/centroid_tracker.py:95-106`;
unset default means never). At 5 detect-fps, `250` ≈ 50 s of stillness, which measured as:
**704 events (4.8 % of all, 25.9 % of never-moved), 163.5 hours (19.5 % of hours), with
36.4 % of the moving events exceeding the same duration.** Given that §6 showed the
never-moved cohort contains real people who sit still, cutting it short at 50 s is a
regression risk bought with hours nobody asked to keep. Leave the default alone until the
size filter has been evaluated.

### The rest, in order of expected value

1. **`min_score` / `threshold`** — the observed score ceiling is 0.844 and a third of
   main_space events are under 0.70, so this is the cheapest blunt instrument. No AUC
   measured yet; do it *after* the area filter so the two effects can be told apart.
2. **Motion cost** — `threshold: 30, contour_area: 10` produces 1.46× the requested detect
   rate, i.e. a free 46 % more chances to be wrong, on a box at load 6.5. Raising
   `contour_area` and `threshold` costs nothing in FP terms and buys headroom.
3. **Masks** — only for spots the operator has *identified*. The recurring coordinates are
   known (e.g. main_space `0.265,0.156` and `0.354,0.898`, social_space `0.233,0.001`), but a
   mask over a bench where people sit is worse than the FP.
4. **Zones** — the untried lever with the best ratio of effect to risk: `inertia` (default 3),
   `loitering_time`, `speed_threshold` (which needs `distances`, and is the only thing that
   populates `average_estimated_speed`), and `review`/`record`/`snapshots` `required_zones`,
   which gate *notifications* without touching detection at all.

## 8. Not applied: the change on the table

Two lines in `/srv/frigate/config/config.yaml`, per camera, with the numbers above in a
comment so the next reader does not re-derive them:

```yaml
# main_space, workshop: objects.filters.person.min_area — measured 2026-11-12 against the
# 30-day event DB (docs/frigate-false-positives-2026-11-12.md §7): drops ~75% of never-moving
# person tracks for ~7% of moving ones. Values are the cohort's p75 box area in DETECT pixels.
```

Then `sudo systemctl restart podman-frigate`, read the live values back from
`http://127.0.0.1:5000/api/config` (rule 21 — do not evaluate to find out what it does), and
re-measure after seven days with the query below. Note that the operator edited
`/srv/frigate/config/config.yaml` at **2026-11-11 19:44** (5,703 → 6,236 bytes) without this
flake's knowledge, so re-read the file before editing; it remains off-flake
(`docs/follow-ups.md`, "frigate's camera config is not in the repo").

## 9. Re-measuring

```bash
sudo sqlite3 /srv/frigate/config/frigate.db ".backup /tmp/fdb.db"
python3 - <<'EOF'
import json, sqlite3
# NEVER-MOVED = path_data span < 0.02 and dur > 300s ; MOVING = span >= 0.08 and 10..240s
db = sqlite3.connect("/tmp/fdb.db"); cfg = None  # cfg = json.load of GET /api/config for detect w/h
for cam, end, data in db.execute("SELECT camera,end_time,data FROM event "
                                 "WHERE label='person' AND end_time IS NOT NULL"):
    d = json.loads(data or "{}"); b = d.get("box")
    if not b: continue
    p = [a[0] for a in (d.get("path_data") or []) if isinstance(a[0], list)]
    xs = [a[0] for a in p]; ys = [a[1] for a in p]
    ext = max(max(xs)-min(xs), max(ys)-min(ys)) if xs else 0.0
    W, H = cfg["cameras"][cam]["detect"]["width"], cfg["cameras"][cam]["detect"]["height"]
    print(cam, ext, b[3]*H, (b[2]*W)*(b[3]*H))   # camera, span, box HEIGHT px, box AREA px^2
EOF
```

Then compare the never-moved share of events and of hours before/after
(`SUM(end_time-start_time)` grouped by the same cohorts), and check `detection_fps` per
camera against its requested `fps` — the 1.5–1.7× overshoot is the other thing a motion tweak
should move.

## 10. Loose ends this document leaves open

- Six crops are still unlabelled (round 1's `cam8` 1,208 s and 897 s; four `woodwork` rows
  the model called "clothing"). They decide whether the ~7 % cost in §7 is acceptable.
  The review sets were `/tmp/vlm-eval/review*` on `services1` — `/tmp`, so they are gone on
  the next reboot; the numbers above are the record.
- `machines/services3/README.md` says the box is "running nothing yet" and has "20 GiB
  free"; both were untrue when read on 2026-11-12 (`llama-server.service` active, 9.3 GB
  free, GC'd to 84 GB the same day). Corrected in `docs/follow-ups.md`.
- The driver clock: `legacy_580` is verified present **at this flake's nixpkgs pin**, and it
  is a branch with an end. Any multi-year bet on that card is a bet on a driver nobody is
  shipping for current kernels.
- Unexplored, cheap and CUDA-free: `whisper-ws` runs on `aibox` (whose only GPU is a Radeon
  660M iGPU, `machines/aibox/alexandria.nix`) and ASR is a Vulkan-shaped workload. Not
  measured; recorded so the next person does not have to re-derive what Pascal is good at.
