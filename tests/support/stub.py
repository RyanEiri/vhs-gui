import sys, os, json, glob, time
tool, state = sys.argv[1], sys.argv[2]
if tool.startswith("realesrgan"):
    tool = "upscaler"
args = sys.argv[3:]
cfgp = os.path.join(state, "config.json")
cfg = json.load(open(cfgp))
with open(os.path.join(state, "calls.txt"), "a") as f:
    f.write("CALL " + tool + "\n")
    for a in args:
        f.write(a + "\n")
os.makedirs(os.path.join(state, "info"), exist_ok=True)
def info_path(n): return os.path.join(state, "info", n + ".json")
def load_info(n):
    try: return json.load(open(info_path(n)))
    except Exception: return None
def once(key, name):
    d = cfg.get(key, {})
    if name in d:
        v = d.pop(name)
        json.dump(cfg, open(cfgp, "w"))
        return v
    return None
j = " ".join(args)
if tool == "ffprobe":
    f = args[-1]
    if "format=duration" in j:
        b = os.path.basename(f)
        if b.startswith("seg_"):
            i = load_info(b)
            if i is None:
                sys.exit(1)
            print(i["dur"])
        else:
            print(cfg["duration"])
    elif "stream=r_frame_rate" in j: print(cfg["fps"])
    elif "stream=width" in j: print(cfg["w"])
    elif "stream=height" in j: print(cfg["h"])
    elif "stream=index" in j:
        if cfg.get("audio", True): print("1")
    elif "nb_read_packets" in j:
        i = load_info(os.path.basename(f)); print(i["frames"] if i else 0)
    sys.exit(0)
if tool == "ffmpeg":
    out = args[-1]
    if out.endswith("frame_%08d.jpg"):
        d = os.path.dirname(out); os.makedirs(d, exist_ok=True)
        for k in range(1, cfg["frames"] + 1):
            open(os.path.join(d, "frame_%08d.jpg" % k), "wb").write(b"jpg")
        open(os.path.join(state, "cur_len"), "w").write(args[args.index("-t") + 1])
    elif "-framerate" in args:
        src = os.path.dirname(args[args.index("-i") + 1])
        n = len(glob.glob(os.path.join(src, "*.jpg"))) - cfg.get("rebuild_short_by", 0)
        b = os.path.basename(out)
        dur = once("dur_override_once", b)
        if dur is None:
            dur = float(open(os.path.join(state, "cur_len")).read())
        open(out, "wb").write(b"mp4")
        json.dump({"frames": n, "dur": dur}, open(info_path(b), "w"))
    elif "concat" in args:
        open(out, "wb").write(b"concat")
    else:
        open(out, "wb").write(b"final")
    sys.exit(0)
if tool == "upscaler":
    i = args[args.index("-i") + 1]; o = args[args.index("-o") + 1]
    open(os.path.join(state, "upscaler_started"), "w").write(os.environ.get("BATCH_SIZE", ""))
    while os.path.exists(os.path.join(state, "hold")):
        time.sleep(0.05)
    os.makedirs(o, exist_ok=True)
    names = sorted(os.listdir(i))
    for nm in names[: len(names) - cfg.get("upscaler_short_by", 0)]:
        open(os.path.join(o, nm), "wb").write(b"up")
    sys.exit(0)
