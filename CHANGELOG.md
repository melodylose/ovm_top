# Changelog

## Unreleased

### Workspace UI / UX Steps 2–8

- 新增 `Workspace`、`InputMode` 與帶 identity 的互斥 `DetailTarget`，並保留 `h/l/u/d/o/4/q` 舊快捷鍵。
- UI 改為 Overview、Domains、Network、Disk、Logs、FC/SAN 的 full-screen workspace；`0` 返回 Overview，`1`–`5` 保留既有區域切換語意。
- Overview 顯示 Xen host、Domain、Network、Disk 與 FC/SAN 健康摘要。
- Domains Workspace 以 DomID 作為 detail identity，name 僅作 display metadata；VM-centric tree 僅顯示 host-side 可證實的 VIF、VBD、WWID 與 multipath 關係。
- FC/SAN Workspace 新增 Overview、Ports、Targets、Multipath subviews，保留 summary/detail、map preview、path health 與 visible ranges。
- Network 與 Disk Workspace 分離 performance/topology view；identity 與 storage scope 明確標示為 host-observed/host-side，未查詢 array metadata。
- Logs 升級為 full-screen、memory-only Workspace，支援 level、source、case-insensitive text 與 1m/5m/15m/1h time-range filters；`c` 清除 view filters，`C` 才清除 memory buffer。
- 新增 large/medium/small responsive detail layout，以及 `?` context help overlay；所有主要 list 保留 visible range/total。
- Persistent Logs 寫入 `~/.local/state/ovm-top/logs/`，依 UTC 日期使用 `YYYY-MM-DD.jsonl` JSONL 檔案。
- 單檔上限 20 MiB，使用不覆蓋既有資料的遞增 suffix rotation，並保留最近 7 個 UTC 日曆日。
- Persistent log 檔案強制使用 Unix `0600` permission；初始化、rotation 或寫入失敗時安全降級為 memory-only，warning 直接輸出 stderr，避免 recursive logging 與 UI 中斷。
- Startup 會載入 retention 範圍內的 daily/rotation JSONL，解析 escaped message 後依 timestamp 排序，並將最新 1000 筆回填 Logs memory buffer；個別檔案或資料列損壞只輸出 warning，不停用後續 persistent writing。

### UI Focus 與互動

- Log 面板預設隱藏，避免佔用主畫面空間。
- 新增面板 Focus 編號：`1` Domains、`2` Host Network、`3` Host Disk I/O、`4` Logs。
- 新增數字鍵切換面板 Focus。
- 新增 `j/k` 快捷鍵，依目前 Focus 捲動資料列表。
- 顯示目前 Focus 面板與快捷鍵提示。
- Log 面板支援獨立捲動、filter、清除與匯出。

### App 模組

- 新增 `Focus` 狀態。
- 新增 Domains、Network、Disk、Logs 各面板的內部 scroll 狀態。
- Log 預設狀態改為隱藏。

### UI 模組

- 更新 Domains、Network、Disk、Logs 面板標題與 Focus 標示。
- 根據 Log 面板顯示狀態動態調整 layout。
- 加入面板 Focus 與 scroll 操作提示。

### Main 模組

- 新增數字鍵 Focus 切換。
- 新增 `j/k` Table 捲動控制。
- 調整 `f/c/s` 僅在 Logs Focus 時執行。

### Collector 模組

- 保留 Disk I/O、Network、Xentop collector 的資料更新架構。
- 各 collector 的錯誤與狀態事件持續透過 Log channel 傳送至 UI。
- 修正 xentop 重複／截斷 NAME 導致不同 Domain 資料互相覆蓋的問題。
- xentop snapshot 改以輸出批次保留 rows，merge 時在名稱不匹配時使用穩定 stream order fallback。
- Domain collector 偵測 duplicate identity 並透過 Domain Warning Log 提示。
- xentop collector 啟用 full-name 輸出，使用完整 Domain name 對應 `xm list` inventory。
- Domain merge 新增 healthy、order fallback 與 unmatched diagnostics。

### Domains Reliability

- 修正新 Hypervisor 上 `xentop` Domain name 被截斷，造成多個 Domain realtime 資料互相覆蓋的問題。
- 啟用 xentop full-name 輸出，使用完整 Domain name 對應 `xm list` 的 Domain inventory。
- 以 snapshot row 保留同名輸出，並在 identity 不匹配時使用 stream order fallback。
- 新增 merge healthy、fallback、inventory unmatched 與 realtime unmatched diagnostics。
- Domain 資料更新已於另一台 Hypervisor 驗證可正常顯示。
- 新增 Domain snapshot generation、collection time 與 freshness status 基礎模型。
- Domains panel 顯示 `LIVE`、`STALE`、`FALLBACK` 與 `NO DATA` 狀態。
- 建立初版 topology snapshot module，為後續 VM-to-Storage relationship layer 預留介面。
- 架構分析草稿改列入 `.gitignore`，僅保留於本機，不納入版本控制。
- 建立 `topology` snapshot、Domain identity、VBD/VIF 與 host network relationship 基礎模型。
- 新增 xenstore VBD/VIF reader 與 `/sys/class/net` bridge/bond topology reader。
- 新增 Multipath `Healthy`、`Degraded`、`Failed`、`Unknown` health model。
- Topology collector 開始定期收集 Domain identity、VBD、VIF、bridge、bond 與 physical NIC 關係。
- Domain panel 顯示目前 topology snapshot 的 VBD/VIF 數量。
- Multipath Summary/Detail 顯示 path health 狀態。
- VBD topology 會嘗試解析 physical-device、host block device 與 multipath dm UUID/WWID。
- VIF topology 會解析 MAC 與 `/sys/class/net/<vif>/brport/bridge` bridge 關係。
- Topology snapshot 顯示 Storage map 數量，Domains panel 顯示 VBD/VIF/Multipath topology counts。
- Domains Focus 新增 `l` 開啟 Domain Detail tree，`h`/`Esc` 返回 Summary。
- Domain Detail 顯示 DomID、VIF/bridge、VBD、host block device 與可辨識的 WWID。
- VBD mapping 新增 `Exact`、`Derived`、`Fallback`、`Unknown` confidence 標示。
- WWID 解析優先使用 `/dev/disk/by-id/scsi-*`，再 fallback 至 dm UUID。
- `by-id/scsi-*` mapping 標示為 `Exact`；dm UUID 標示為 `Derived` 或 `Fallback`，不再誤標為 exact。
- Domain Detail 會在 VBD 下方顯示對應 multipath mapper、path health 與 Host-side scope。
- Network Focus 新增 `l` 開啟 Network Detail tree，顯示 bridge、bond、master 與 member 關係。
- Network Detail 使用獨立 scroll state，`h`/`Esc` 返回 Network Summary。
- Storage topology 明確限制為 Host-side，涵蓋 VBD、block device、WWID、dm-X、multipath 與 FC path。
- 移除並禁止 SCV3020 API/CLI、Storage Manager 與 Array-side metadata 接入；不可取得資料以 Unknown/Unavailable 表示。

### Table Scrolling

- Domains、Network、Disk 與 Logs 保留內部資料捲動能力。
- 移除畫面上的垂直 scrollbar，避免 scrollbar Rect 影響 Table layout。
- 修正 `j/k` 捲動超過資料範圍後顯示空白畫面的問題。
- 保留 total、viewport、offset 與 max_offset 的邊界計算。

### Focus 與 Table Scrolling

- Focus 面板新增黃色高亮外框與粗體標題，提升目前視窗的辨識度。
- 統一 Table 的 total、viewport 與 offset 計算。
- `j/k` 捲動位置變化會寫入互動式 UI Log。
- Focus 切換與 scrollbar 到達頂端/底端會寫入 UI Log。
- 避免重複記錄相同 scrollbar 邊界事件。

### Log Scrolling 數值修正

- 新增 Logs scroll state 的 `total`、`viewport`、`offset` 與 `max_offset` 診斷資訊。
- 修正新增互動式 UI Log 後 scroll position 不再停留在 bottom 的問題。
- Logs 位於 bottom 時新增內容會自動跟隨最新位置。
- 使用者檢視舊 Log 時不會因新 Log 到達而強制跳到底部。
- 補充 scrollbar 邊界與新增 Log 行為測試。

### Table Geometry 修正

- 統一 Table body 的 viewport 高度計算。
- 移除 scrollbar 專用 Rect，避免影響 Table layout。
- 新增 Table body viewport 測試，避免 border/header 被錯誤計入可見列數。

### FC/SAN

- 新增 FC host、FC target 與 Compellent multipath map collector。
- 新增 `[5] FC/SAN` Focus，預設顯示 Multipath map。
- 預設隱藏 local PERC `dm-0`，只顯示 Compellent map。
- 將 `dm-*` mapper 的 Disk I/O 合併至 multipath map。
- 新增 Ports、Targets、Multipath 顯示模式設定選單。
- Path detail 預設隱藏，保留資料模型供後續擴充。
- 暫不實作 Domain-to-LUN mapping，保留後續設計空間。
- 將 `FC-Collect-Simple.md` 排除於 Git 版本控制之外。

### FC/SAN Detail View

- 保留 Multipath summary 作為預設顯示模式。
- 新增 FC/SAN map detail view 與目前 map 選取狀態。
- 新增 `l` 開啟目前 multipath map 的 path detail。
- 新增 `h`/`Esc` 返回 Multipath summary。
- 顯示 H:C:T:L、device、major/minor 與 path state。
- Detail view 顯示 map-level Disk I/O。
- FC/SAN 設定選單可切換 Path detail 顯示狀態，預設為隱藏。
- Domain-to-LUN mapping 仍保留至後續階段。
- Summary 模式新增目前 WWID row 高亮與獨立選取狀態。
- `j/k` 先移動 map selection，再同步調整 viewport offset。
- Detail 模式會清除父層 map selection。
- Detail 頁面改為 FC/SAN tree 結構，父 map 資訊不再放入視窗標題。
- 修正 FC/SAN Summary 與 Detail 共用 panel 時的內容重疊問題。
- Detail render 前清除 panel area，Summary 與 Detail 改為互斥顯示。
- FC/SAN Detail 改為獨立左右 panel，左側保留精簡 Summary，右側顯示 Detail tree。
- 窄終端機自動切換為 Detail-only layout，避免 Summary 與 Detail 共用顯示區域。
- 重新整理快捷鍵：`j/k` 為下／上，`l/h` 顯示／隱藏 Detail，`u/d` 切換 Detail preview。
- `4` 統一控制 Logs 顯示與隱藏。
- 修正 Detail Preview 時 Summary 父層高亮回到第一個 map 的問題。
- Detail Preview 會同步 Summary context 與 viewport，返回 Summary 後保留最後查看的 WWID。
- Domain、Network 與 FC Detail navigation 新增共用 `DetailTarget` 狀態，避免多個 Detail overlay 同時開啟。
- Topology collector 在 xenstore、network 或 multipath 部分讀取失敗時改標示 `STALE`，不再將不完整資料標示為 `LIVE`。
- Domains 標題同步顯示 Domain snapshot 與 Topology snapshot freshness 狀態。
- 新增 `STORAGE` log source，記錄 multipath topology 讀取失敗。
- Domains、Network、Disk、FC/SAN、Logs 與 Detail tree 標題新增 visible range / total，避免 viewport 截斷造成資料不存在的誤判。
- `ovm-top_ui_ux_improvement_draft.md` 列入 `.gitignore`，僅作為本機 UI/UX 規劃草稿。
