//! ファイルリスト内のナビゲーション方向を表す型。

/// ナビゲーションの方向 (PendingContainer 展開時の current_index 配置に使う)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavigationDirection {
    /// 前進方向 (次へ・先頭へ・次のフォルダへ等)。展開後グループの先頭に配置する
    Forward,
    /// 後退方向 (前へ・末尾へ等)。展開後グループの末尾に配置する
    Backward,
}
