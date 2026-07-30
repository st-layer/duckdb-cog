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
        elif body.get("collections") == ["many"]:
            # 합성 대용량: 600행 × 2페이지 = 1,200행 — 클라이언트 기본 행 상한
            # (1,000) 초과를 유도하는 무음-절단 계약(필드 리포트 2차 ①) 검증용
            page = 2 if "token" in body else 1
            doc = {
                "type": "FeatureCollection",
                "features": [
                    {
                        "type": "Feature",
                        "id": f"many-{page}-{i:03d}",
                        "assets": {"B04": {"href": f"https://example.com/many-{page}-{i:03d}.tif"}},
                    }
                    for i in range(600)
                ],
            }
            if page == 1:
                doc["links"] = [{
                    "rel": "next", "href": "{BASE}/search",
                    "method": "POST", "merge": True, "body": {"token": "page:2"},
                }]
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
