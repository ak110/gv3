mod association;
mod context_menu;
mod sendto;

use anyhow::Result;

/// 全てのシェル統合を登録する
pub fn register_all() -> Result<()> {
    println!("シェル統合を登録しています...");

    association::register()?;
    println!("  ファイル関連付けを登録しました");

    context_menu::register()?;
    println!("  コンテキストメニューを登録しました");

    sendto::register()?;
    println!("  「送る」を登録しました");

    association::notify_shell();
    println!("シェル統合の登録が完了しました");
    Ok(())
}

/// 全てのシェル統合を解除する
pub fn unregister_all() -> Result<()> {
    println!("シェル統合を解除しています...");

    association::unregister()?;
    println!("  ファイル関連付けを解除しました");

    context_menu::unregister()?;
    println!("  コンテキストメニューを解除しました");

    sendto::unregister()?;
    println!("  「送る」を解除しました");

    association::notify_shell();
    println!("シェル統合の解除が完了しました");
    Ok(())
}
