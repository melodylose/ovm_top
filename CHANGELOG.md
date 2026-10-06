# Changelog

## Unreleased

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
- 新增 `Enter/d` 開啟目前 multipath map 的 path detail。
- 新增 `Esc` 返回 Multipath summary。
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
