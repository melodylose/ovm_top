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
