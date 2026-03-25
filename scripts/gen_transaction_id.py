#!/usr/bin/env python3
"""Generate X-Client-Transaction-Id for X GraphQL API requests.

Usage: python3 gen_transaction_id.py <METHOD> <PATH> <CT0> <AUTH_TOKEN>
Outputs the transaction ID to stdout.

Based on: https://github.com/iSarabjitDhiman/XClientTransaction (MIT)
"""
import sys, re, math, time, random, base64, hashlib, json
from functools import reduce
from html.parser import HTMLParser

ADDITIONAL_RANDOM_NUMBER = 3
DEFAULT_KEYWORD = "obfiowerehiring"
ON_DEMAND_FILE_REGEX = re.compile(r""",(\d+):["']ondemand\.s["']""", re.VERBOSE | re.MULTILINE)
ON_DEMAND_HASH_PATTERN = r',{}:\"([0-9a-f]+)\"'
INDICES_REGEX = re.compile(r"""(\(\w{1}\[(\d{1,2})\],\s*16\))+""", re.VERBOSE | re.MULTILINE)

# --- Minimal HTML parsing (no BS4 needed) ---

class MetaExtractor(HTMLParser):
    """Extract twitter-site-verification and loading-x-anim SVGs from HTML."""
    def __init__(self):
        super().__init__()
        self.verification_key = None
        self.anim_svgs = {}  # id -> svg content
        self._in_svg = None
        self._svg_depth = 0
        self._svg_content = []

    def handle_starttag(self, tag, attrs):
        d = dict(attrs)
        if tag == "meta" and d.get("name") == "twitter-site-verification":
            self.verification_key = d.get("content", "")
        # Track SVG elements with loading-x-anim IDs
        if self._in_svg is not None:
            self._svg_depth += 1
            attr_str = " ".join(f'{k}="{v}"' for k, v in attrs)
            self._svg_content.append(f"<{tag} {attr_str}>")
        elif tag == "svg":
            svg_id = d.get("id", "")
            if svg_id.startswith("loading-x-anim"):
                self._in_svg = svg_id
                self._svg_depth = 1
                self._svg_content = []

    def handle_endtag(self, tag):
        if self._in_svg is not None:
            self._svg_content.append(f"</{tag}>")
            self._svg_depth -= 1
            if self._svg_depth <= 0:
                self.anim_svgs[self._in_svg] = "".join(self._svg_content)
                self._in_svg = None

    def handle_data(self, data):
        if self._in_svg is not None:
            self._svg_content.append(data)

def get_path_d(svg_content):
    """Extract d attribute from second path in SVG."""
    paths = re.findall(r'<path[^>]*\sd="([^"]*)"', svg_content)
    if len(paths) >= 2:
        return paths[1]
    return paths[0] if paths else ""

# --- Math helpers ---

def js_round(num):
    x = math.floor(num)
    if (num - x) >= 0.5:
        x = math.ceil(num)
    return int(math.copysign(x, num))

def solve(value, min_val, max_val, rounding):
    result = value * (max_val - min_val) / 255 + min_val
    return math.floor(result) if rounding else round(result, 2)

def is_odd(num):
    return -1.0 if num % 2 else 0.0

def float_to_hex(x):
    result = []
    quotient = int(x)
    fraction = x - quotient
    while quotient > 0:
        quotient_new = int(x / 16)
        remainder = int(x - float(quotient_new) * 16)
        result.insert(0, chr(remainder + 55) if remainder > 9 else str(remainder))
        x = float(quotient_new)
        quotient = quotient_new
    if fraction == 0:
        return "".join(result)
    result.append(".")
    while fraction > 0:
        fraction *= 16
        integer = int(fraction)
        fraction -= float(integer)
        result.append(chr(integer + 55) if integer > 9 else str(integer))
    return "".join(result)

def interpolate(from_list, to_list, f):
    return [fv * (1 - f) + tv * f for fv, tv in zip(from_list, to_list)]

def convert_rotation_to_matrix(rotation):
    rad = math.radians(rotation)
    return [math.cos(rad), -math.sin(rad), math.sin(rad), math.cos(rad)]

class Cubic:
    def __init__(self, curves):
        self.curves = curves
    def get_value(self, t):
        if t <= 0:
            if self.curves[0] > 0:
                return (self.curves[1] / self.curves[0]) * t
            if self.curves[1] == 0 and self.curves[2] > 0:
                return (self.curves[3] / self.curves[2]) * t
            return 0
        if t >= 1:
            if self.curves[2] < 1:
                return 1.0 + ((self.curves[3] - 1) / (self.curves[2] - 1)) * (t - 1)
            if self.curves[2] == 1 and self.curves[0] < 1:
                return 1.0 + ((self.curves[1] - 1) / (self.curves[0] - 1)) * (t - 1)
            return 1.0
        lo, hi, mid = 0.0, 1.0, 0.5
        while lo < hi:
            mid = (lo + hi) / 2
            x_est = self.calc(self.curves[0], self.curves[2], mid)
            if abs(t - x_est) < 0.00001:
                return self.calc(self.curves[1], self.curves[3], mid)
            if x_est < t: lo = mid
            else: hi = mid
        return self.calc(self.curves[1], self.curves[3], mid)
    @staticmethod
    def calc(a, b, m):
        return 3*a*(1-m)*(1-m)*m + 3*b*(1-m)*m*m + m*m*m

# --- Main logic ---

def generate(method, path, ct0, auth_token):
    import urllib.request

    cookie = f"ct0={ct0}; auth_token={auth_token}"
    headers = {
        "User-Agent": "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/146.0.0.0 Safari/537.36",
        "Accept": "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        "Accept-Language": "en-US,en;q=0.9",
        "Cookie": cookie,
    }

    # 1. Fetch homepage
    req = urllib.request.Request("https://x.com", headers=headers)
    with urllib.request.urlopen(req) as resp:
        html = resp.read().decode("utf-8", errors="replace")

    # 2. Extract verification key and SVGs
    parser = MetaExtractor()
    parser.feed(html)
    key = parser.verification_key
    if not key:
        print(json.dumps({"error": "No twitter-site-verification found"}), file=sys.stderr)
        sys.exit(1)
    key_bytes = list(base64.b64decode(key.encode()))

    # 3. Get ondemand.s file for indices
    od_match = ON_DEMAND_FILE_REGEX.search(html)
    if not od_match:
        print(json.dumps({"error": "No ondemand.s index found"}), file=sys.stderr)
        sys.exit(1)
    od_index = od_match.group(1)
    hash_regex = re.compile(ON_DEMAND_HASH_PATTERN.format(od_index))
    hash_match = hash_regex.search(html)
    if not hash_match:
        print(json.dumps({"error": "No ondemand.s hash found"}), file=sys.stderr)
        sys.exit(1)
    od_url = f"https://abs.twimg.com/responsive-web/client-web/ondemand.s.{hash_match.group(1)}a.js"

    req2 = urllib.request.Request(od_url, headers=headers)
    with urllib.request.urlopen(req2) as resp2:
        od_text = resp2.read().decode("utf-8", errors="replace")

    # 4. Extract indices from ondemand file
    indices = [int(m.group(2)) for m in INDICES_REGEX.finditer(od_text)]
    if not indices:
        print(json.dumps({"error": "No KEY_BYTE indices in ondemand file"}), file=sys.stderr)
        sys.exit(1)
    row_index_idx = indices[0]
    key_bytes_indices = indices[1:]

    # 5. Get animation key from SVGs
    sorted_ids = sorted(parser.anim_svgs.keys())
    frame_idx = key_bytes[5] % 4
    if frame_idx >= len(sorted_ids):
        print(json.dumps({"error": f"SVG frame index {frame_idx} out of range ({len(sorted_ids)} SVGs)"}), file=sys.stderr)
        sys.exit(1)

    svg_content = parser.anim_svgs[sorted_ids[frame_idx]]
    d_attr = get_path_d(svg_content)
    # Parse: M<x> <y> C ... C ...
    parts = d_attr.split("C")
    first = re.sub(r"[^\d]+", " ", parts[0]).strip().split() if parts else []
    arr = []
    for part in parts:
        nums = [int(x) for x in re.sub(r"[^\d]+", " ", part).strip().split()]
        arr.append(nums)

    # Animation calculation
    total_time = 4096
    ri = key_bytes[row_index_idx] % 16
    frame_time = reduce(lambda a, b: a * b, [key_bytes[i] % 16 for i in key_bytes_indices])
    frame_time = js_round(frame_time / 10) * 10

    if ri >= len(arr):
        ri = ri % max(len(arr), 1)
    frame_row = arr[ri]
    target_time = float(frame_time) / total_time

    # Animate
    from_color = [float(x) for x in [*frame_row[:3], 1]]
    to_color = [float(x) for x in [*frame_row[3:6], 1]]
    to_rotation = [solve(float(frame_row[6]), 60.0, 360.0, True)]
    curves = [solve(float(item), is_odd(i), 1.0, False) for i, item in enumerate(frame_row[7:])]
    cubic = Cubic(curves)
    val = cubic.get_value(target_time)
    color = [max(0, min(255, v)) for v in interpolate(from_color, to_color, val)]
    rotation = interpolate([0.0], to_rotation, val)
    matrix = convert_rotation_to_matrix(rotation[0])

    str_arr = [format(round(v), 'x') for v in color[:-1]]
    for v in matrix:
        rv = round(v, 2)
        if rv < 0: rv = -rv
        hx = float_to_hex(rv)
        str_arr.append(f"0{hx}".lower() if hx.startswith(".") else hx if hx else "0")
    str_arr.extend(["0", "0"])
    animation_key = re.sub(r"[.-]", "", "".join(str_arr))

    # 6. Generate transaction ID
    time_now = math.floor((time.time() * 1000 - 1682924400 * 1000) / 1000)
    time_bytes = [(time_now >> (i * 8)) & 0xFF for i in range(4)]
    hash_val = hashlib.sha256(f"{method}!{path}!{time_now}{DEFAULT_KEYWORD}{animation_key}".encode()).digest()
    hash_bytes = list(hash_val)
    rand = random.randint(0, 255)
    out = bytearray([rand, *[item ^ rand for item in [*key_bytes, *time_bytes, *hash_bytes[:16], ADDITIONAL_RANDOM_NUMBER]]])
    tid = base64.b64encode(out).decode().rstrip("=")

    print(tid)

if __name__ == "__main__":
    if len(sys.argv) != 5:
        print(f"Usage: {sys.argv[0]} <METHOD> <PATH> <CT0> <AUTH_TOKEN>", file=sys.stderr)
        sys.exit(1)
    generate(sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4])
