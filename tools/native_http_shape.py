"""Bounded, lossy HTTP metadata for local protocol research; no raw values escape."""
from http.client import HTTPException, parse_headers
from io import BytesIO

MAX_HEADER_BYTES = 8192
METHODS = {b"GET", b"POST", b"PUT", b"DELETE", b"PATCH", b"HEAD", b"OPTIONS"}
ROUTES = {
    b"/v1/user/login": "login",
    b"/v1/verify/sms-anon": "sms",
    b"/v1/user/login_device_change": "device_confirm",
    b"/v1/user/get-change-device-verify": "device_challenge",
    b"/v1/user/RefreshToken": "refresh",
}
HEADERS = ("authorization", "cookie", "content-length", "content-type",
           "transfer-encoding", "content-encoding", "host", "user-agent")
MEDIA_TYPES = {"application/json", "application/octet-stream",
               "application/x-protobuf", "application/protobuf",
               "application/x-www-form-urlencoded"}


def h2_shape(data):
    """Recognize complete candidate frames only; not a negotiated H2 validator.

    RFC 9113 section 4.1. Never return stream IDs, flags or payload bytes.
    A TLS write can split a frame, so a negative result does not rule out H2.
    """
    if not data or len(data) > MAX_HEADER_BYTES:
        return {"state": "unclassified"}
    names = ("data", "headers", "priority", "rst_stream", "settings",
             "push_promise", "ping", "goaway", "window_update", "continuation")
    offset = 0
    counts = {}
    while offset < len(data):
        if len(data) - offset < 9:
            return {"state": "unclassified"}
        size = int.from_bytes(data[offset:offset+3], "big")
        kind = data[offset+3]
        flags = data[offset+4]
        stream = int.from_bytes(data[offset+5:offset+9], "big") & 0x7fffffff
        if kind >= len(names) or offset + 9 + size > len(data):
            return {"state": "unclassified"}
        if kind in (0, 1, 2, 3, 5, 9) and stream == 0:
            return {"state": "unclassified"}
        if kind in (4, 6, 7) and stream != 0:
            return {"state": "unclassified"}
        if ((kind == 2 and size != 5) or (kind in (3, 8) and size != 4)
                or (kind == 6 and size != 8) or (kind == 7 and size < 8)
                or (kind == 4 and (size % 6 or (flags & 1 and size)))):
            return {"state": "unclassified"}
        counts[names[kind]] = counts.get(names[kind], 0) + 1
        offset += 9 + size
    return {"state": "h2_frame_candidate", "types": counts}


def request_shape(data):
    data = data[:MAX_HEADER_BYTES]
    end = data.find(b"\r\n\r\n")
    if end < 0:
        return {"state": "incomplete_header"}
    first, separator, headers = data[:end].partition(b"\r\n")
    parts = first.split(b" ")
    if len(parts) != 3 or parts[0] not in METHODS or parts[2] not in (b"HTTP/1.0", b"HTTP/1.1"):
        return {"state": "invalid_request_line"}
    try:
        parsed = parse_headers(BytesIO(headers + b"\r\n\r\n"))
    except (HTTPException, ValueError):
        return {"state": "invalid_header"}
    if parsed.defects:
        return {"state": "invalid_header"}
    content_type = parsed.get_content_type() if "content-type" in parsed else "absent"
    if content_type not in MEDIA_TYPES and content_type != "absent":
        content_type = "other"
    return {
        "state": "complete_header",
        "method": parts[0].decode("ascii"),
        "route": ROUTES.get(parts[1].split(b"?", 1)[0], "other"),
        "present_headers": [name for name in HEADERS if name in parsed],
        "content_type": content_type,
        "unknown_header_count": sum(name.lower() not in HEADERS for name in parsed.keys()),
    }
