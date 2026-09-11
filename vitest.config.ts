import { defineConfig } from 'vitest/config';

// 独立配置：单元测试只覆盖 src/lib 下的纯函数，不需要 SvelteKit 插件
export default defineConfig({
  test: {
    include: ['src/lib/**/*.test.ts'],
    environment: 'node',
  },
});
