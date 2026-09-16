import json
import unittest

from native_http_shape import MAX_HEADER_BYTES, request_shape, h2_shape


class RequestShapeTests(unittest.TestCase):
    def test_h2_coalesced_and_truncated(self):
        frame = b'\x00\x00\x06\x00\x01\x00\x00\x00\x01secret'
        result = h2_shape(frame + frame)
        self.assertEqual(result, {"state": "h2_frame_candidate", "types": {"data": 2}})
        self.assertNotIn("secret", json.dumps(result))
        for end in range(len(frame)):
            self.assertEqual(h2_shape(frame[:end]), {"state": "unclassified"})

    def test_h2_invalid_stream_and_fixed_length(self):
        self.assertEqual(h2_shape(bytes(9)), {"state": "unclassified"})
        self.assertEqual(h2_shape(b'\x00\x00\x00\x06\x00\x00\x00\x00\x00'),
                         {"state": "unclassified"})

    def test_credentials_query_body_and_unknown_names_never_escape(self):
        secret = "synthetic-secret-marker"
        data = (f"POST /v1/user/login?token={secret} HTTP/1.1\r\n"
                f"Host: {secret}\r\nAuthorization: Bearer {secret}\r\n"
                f"Cookie: session={secret}\r\nX-{secret}: {secret}\r\n"
                f"Content-Type: application/json; secret={secret}\r\n\r\n{secret}").encode()
        result = request_shape(data)
        self.assertNotIn(secret, json.dumps(result))
        self.assertEqual(result["route"], "login")
        self.assertEqual(result["content_type"], "application/json")
        self.assertEqual(result["unknown_header_count"], 1)

    def test_partial_header_does_not_claim_complete(self):
        self.assertEqual(request_shape(b"POST / HTTP/1.1\r\nCookie: private"),
                         {"state": "incomplete_header"})

    def test_header_beyond_limit_is_not_parsed(self):
        data = b"GET / HTTP/1.1\r\nX: " + b"a" * MAX_HEADER_BYTES + b"\r\n\r\n"
        self.assertEqual(request_shape(data), {"state": "incomplete_header"})

    def test_unknown_route_and_media_type_are_labels(self):
        result = request_shape(b"GET /private-user HTTP/1.1\r\nContent-Type: private/type\r\n\r\n")
        self.assertEqual(result["route"], "other")
        self.assertEqual(result["content_type"], "other")
        self.assertNotIn("private", json.dumps(result))

    def test_invalid_request_line(self):
        self.assertEqual(request_shape(b"POST secret HTTP/9\r\n\r\n"),
                         {"state": "invalid_request_line"})

    def test_duplicate_sensitive_headers_emit_presence_only(self):
        result = request_shape(b"GET / HTTP/1.1\r\nCookie: one\r\nCookie: two\r\n\r\n")
        self.assertEqual(result["present_headers"], ["cookie"])


if __name__ == "__main__":
    unittest.main()
