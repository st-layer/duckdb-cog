"""STAC API /search mock — test/sql/read_stac_search.test 전용 (T3, 이슈 #29).

POST /search 만 구현: body 에 token 이 없으면 page1(아이템 2 + POST next
[body+merge]), 있으면 page2(아이템 1, next 없음). collections == ["empty"] 는
빈 페이지 — named 인자가 서버까지 전달되는지의 E2E 검증용. 페이지 JSON 은
test/data/stac/search_page{1,2}.json (계약 문서) — "{BASE}" 를 자기 주소로 치환.

사용: python scripts/mock_stac_api.py PORT DATA_DIR  (justfile ext-test 가 기동)
"""

import json
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

PORT = int(sys.argv[1])
DATA = sys.argv[2]


class Handler(BaseHTTPRequestHandler):
    def do_POST(self):
        if self.path != "/search":
            self.send_error(404)
            return
        n = int(self.headers.get("Content-Length") or 0)
        body = json.loads(self.rfile.read(n) or b"{}")
        if body.get("collections") == ["empty"]:
            doc = {"type": "FeatureCollection", "features": []}
        else:
            name = "search_page2.json" if "token" in body else "search_page1.json"
            with open(f"{DATA}/{name}") as f:
                doc = json.load(f)
        raw = json.dumps(doc).replace("{BASE}", f"http://127.0.0.1:{PORT}").encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/geo+json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def log_message(self, *_):
        pass


if __name__ == "__main__":
    HTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
