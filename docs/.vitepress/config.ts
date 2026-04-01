import { defineConfig } from 'vitepress'

export default defineConfig({
  lang: 'ja',
  title: 'ぐらびゅ',
  description: 'Windows用画像ビューアー',
  base: '/gv/',

  themeConfig: {
    nav: [
      { text: 'ホーム', link: '/' },
      { text: 'はじめに', link: '/getting-started' },
    ],

    sidebar: [
      {
        text: 'ユーザーガイド',
        items: [
          { text: 'はじめに', link: '/getting-started' },
          { text: '画像の表示', link: '/viewing' },
          { text: 'ナビゲーション', link: '/navigation' },
          { text: 'ファイル操作', link: '/file-operations' },
          { text: '画像編集', link: '/editing' },
          { text: '対応フォーマット', link: '/formats' },
          { text: 'カスタマイズ', link: '/customization' },
        ],
      },
      {
        text: '開発者向け',
        items: [
          { text: 'コンセプト', link: '/concept' },
          { text: 'アーキテクチャ', link: '/architecture' },
          { text: '開発ガイド', link: '/development' },
        ],
      },
    ],

    socialLinks: [
      { icon: 'github', link: 'https://github.com/ak110/gv' },
    ],

    search: {
      provider: 'local',
    },

    docFooter: {
      prev: '前のページ',
      next: '次のページ',
    },
    darkModeSwitchLabel: '外観',
    returnToTopLabel: 'トップに戻る',
    outline: {
      label: '目次',
    },
  },
})
