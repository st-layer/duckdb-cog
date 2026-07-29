# README 데모 GIF 재현 (charmbracelet/vhs) — `vhs docs/demo.tape`
#
# 사전 조건:
#   - vhs (brew install vhs)
#   - DuckDB CLI >= 1.5.5 가 PATH 에 (아래 Hide 블록의 PATH 를 환경에 맞게 조정)
#   - 네트워크 (공개 Sentinel-2 버킷 접근, AWS_SKIP_SIGNATURE 는 tape 안에서 지정)
# 출력은 docs/demo.gif 로 덮어쓴다.

Output "docs/demo.gif"

Set Shell "bash"
Set FontSize 15
Set Width 1180
Set Height 720
Set Theme "Catppuccin Mocha"
Set TypingSpeed 14ms
Set Padding 18
Set Framerate 24

Hide
Type "cd /tmp && export PATH=/tmp/duckdb155:$PATH && export PS1='$ ' && clear"
Enter
Sleep 1s
Show

Type "# duckdb-cog: query Cloud-Optimized GeoTIFFs in place, no GDAL, no download"
Enter
Sleep 1200ms

Type "AWS_SKIP_SIGNATURE=true duckdb"
Enter
Sleep 2s

Type "INSTALL cog FROM community;  LOAD cog;"
Enter
Sleep 3s

Type "-- a real 120 MB Sentinel-2 band, sitting in S3"
Enter
Sleep 800ms
Type "SET VARIABLE b04 = 'https://sentinel-cogs.s3.us-west-2.amazonaws.com/sentinel-s2-l2a-cogs/52/S/CG/2026/6/S2B_52SCG_20260630_0_L2A/B04.tif';"
Enter
Sleep 900ms

Type ".timer on"
Enter
Sleep 500ms

Type "-- tile grid + overviews: metadata-only range reads, pixels untouched"
Enter
Sleep 800ms
Type "SELECT level, count(*) AS tiles, any_value(crs) AS crs"
Enter
Type "  FROM read_cog(getvariable('b04')) GROUP BY level ORDER BY level;"
Enter
Sleep 2500ms

Type "-- pixel value and a zonal mean, straight off the object store"
Enter
Sleep 800ms
Type "SELECT RS_Value(getvariable('b04'), 325210, 4121100) AS red,"
Enter
Type "       RS_ZonalStats(getvariable('b04'), [322000,4119000,326400,4123200], 1, 'mean')::INT AS zonal_mean;"
Enter
Sleep 5s

Sleep 1500ms
