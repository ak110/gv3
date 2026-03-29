import { defineConfig } from 'vitepress'

export default defineConfig({
  lang: 'ja',
  title: 'ぐらびゅ',
  description: 'Windows用画像ビューアー',
  base: '/gv/',

  themeConfig: {
    nav: [
      { text: 'ホーム', link: '/' },
      { text: 'ユーザーガイド', link: '/user-guide' },
    ],

    sidebar: [
      {
        text: 'ユーザー向け',
        items: [
          { text: 'コンセプト', link: '/concept' },
          { text: 'ユーザーガイド', link: '/user-guide' },
        ],
      },
      {
        text: '開発者向け',
        items: [
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
