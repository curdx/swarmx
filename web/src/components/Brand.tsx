/**
 * Brand — swarmx 的产品标志(唯一权威实现)。
 *
 * 概念:SwarmX —— 四只 worker 节点连成 X,中心一只调度节点。「swarm
 * 蜂群 + X」双关:图形本身是个 X(产品名),构成方式是蜂群节点连线
 * (产品做的事)。蓝 600 圆角底,白图形,16px 下依然可读。
 *
 * 三处共用,杜绝「应用内一个 logo、favicon 另一个、安装包第三个」:
 *   - 本组件(AppShell 顶栏 / Welcome / 设置→关于)
 *   - web/public/favicon.svg(同一份图形的静态拷贝,浏览器标签页用)
 *   - web/src-tauri/icons/*(由 favicon.svg 渲染出的 1024 PNG 经
 *     `npx tauri icon` 全套再生成 —— 改图形后重跑该命令)
 */

export function BrandMark({
  size = 28,
  className,
}: {
  size?: number;
  className?: string;
}) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 32 32"
      className={className}
      role="img"
      aria-label="swarmx"
    >
      <rect width="32" height="32" rx="7" fill="#2563EB" />
      <g stroke="#fff" strokeWidth="2" strokeLinecap="round">
        <path d="M10 10 22 22M22 10 10 22" />
      </g>
      <g fill="#fff">
        <circle cx="16" cy="16" r="3" />
        <circle cx="10" cy="10" r="2.4" />
        <circle cx="22" cy="10" r="2.4" />
        <circle cx="10" cy="22" r="2.4" />
        <circle cx="22" cy="22" r="2.4" />
      </g>
    </svg>
  );
}
