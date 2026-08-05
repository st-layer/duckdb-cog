"""Range + CORS 정적 서버 — engine-wasm fetch 경로 테스트 전용 (#66 슬라이스 2).

rangehttpserver 의 RangeRequestHandler 에 CORS 를 얹는다: 테스트 페이지
(wasm-bindgen-test 임의 포트)와 교차 출처인 데다 Range 는 CORS-safelisted
헤더가 아니어서 preflight(OPTIONS) 가 온다.

사용: python scripts/range_cors_server.py PORT DIR  (justfile wasm-test 가 기동)
"""

import sys
from functools import partial
from http.server import HTTPServer

from RangeHTTPServer import RangeRequestHandler

PORT = int(sys.argv[1])
DIR = sys.argv[2]


class Handler(RangeRequestHandler):
    def end_headers(self):
        self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header(
            "Access-Control-Expose-Headers",
            "Content-Range, Accept-Ranges, Content-Length",
        )
        super().end_headers()

    def do_OPTIONS(self):  # noqa: N802 — http.server 관례
        self.send_response(204)
        self.send_header("Access-Control-Allow-Methods", "GET, OPTIONS")
        self.send_header("Access-Control-Allow-Headers", "Range")
        self.end_headers()

    def log_message(self, *args):  # 테스트 로그 소음 억제 (파일로만)
        pass


HTTPServer(("127.0.0.1", PORT), partial(Handler, directory=DIR)).serve_forever()
