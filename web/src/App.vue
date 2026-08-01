<script setup>
import { darkTheme, zhCN, dateZhCN } from 'naive-ui';

// prototype palette: graphite dark + teal accent (assets/app.css)
const themeOverrides = {
  common: {
    primaryColor: '#19b8a6',
    primaryColorHover: '#2fc6b5',
    primaryColorPressed: '#15a292',
    bodyColor: '#14171c',
    cardColor: '#1b1f27',
    modalColor: '#1b1f27',
    popoverColor: '#1b1f27',
    borderRadius: '8px',
  },
};
</script>

<template>
  <n-config-provider :theme="darkTheme" :theme-overrides="themeOverrides" :locale="zhCN" :date-locale="dateZhCN">
    <n-loading-bar-provider>
      <n-message-provider>
        <n-dialog-provider>
          <router-view />
        </n-dialog-provider>
      </n-message-provider>
    </n-loading-bar-provider>
  </n-config-provider>
</template>

<style>
html, body, #app { height: 100%; margin: 0; }

/* design tokens mirrored from the prototype's assets/app.css */
:root {
  --bg: #14171c;
  --surface: #1b1f27;
  --surface-warm: #252b35;
  --accent: #19b8a6;
  --accent-2: #8b93a3;
  --success: #3ecf8e;
  --warn: #e0a83e;
  --danger: #e5645f;
  --muted: #7b8493;
  --fg: #e8ebf0;
  --fg-2: #aab2bf;
}

/* Chrome 自动填充会强行给 input 刷上 rgb(232,240,254) 的浅底,在深色主题里就是
   一块白斑。UA 那层背景改不掉,但可以把它裁剪到文字轮廓上 —— 于是露出的仍是
   naive-ui 输入框自己的底色,不用去猜合成后的具体色值。 */
input:-webkit-autofill,
input:-webkit-autofill:hover,
input:-webkit-autofill:focus,
input:-webkit-autofill:active,
textarea:-webkit-autofill,
select:-webkit-autofill {
  -webkit-background-clip: text;
  background-clip: text;
  -webkit-text-fill-color: var(--fg);
  caret-color: var(--fg);
}

/* slim rounded scrollbars on the dark theme (native square bars look off) */
* {
  scrollbar-width: thin;                      /* Firefox */
  scrollbar-color: #3a4250 transparent;
}
::-webkit-scrollbar {
  width: 9px;
  height: 9px;
}
::-webkit-scrollbar-track,
::-webkit-scrollbar-corner {
  background: transparent;
}
::-webkit-scrollbar-thumb {
  background: #3a4250;
  border-radius: 999px;
  border: 2px solid transparent;              /* inset gutter */
  background-clip: padding-box;
}
::-webkit-scrollbar-thumb:hover {
  background: #4a5466;
  border: 2px solid transparent;
  background-clip: padding-box;
}
</style>
