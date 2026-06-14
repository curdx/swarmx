/**
 * mcpCatalog — curated「快捷安装 MCP」目录种子。
 *
 * 数据经 fan-out 联网搜索 + 对抗校验得出（mid-2026，命令可直接复制粘贴）。
 * 两个易踩的事实：
 *   ① GitHub 官方已改为**远程 http** server（不再是本地 npx）。
 *   ② MCP `sse` 传输 2026-04 起停用，远程一律 `type:"http"`（streamable HTTP）。
 *
 * 图标只用通用 Lucide 图标——lucide-react@1.x 做过品牌图标清洗，
 * `Github`/`Chrome`/`Figma` 等可能已移除，用品牌图标会 build 失败。
 */

import {
  Brain,
  Bug,
  Cloud,
  Compass,
  CreditCard,
  Database,
  FolderTree,
  Gauge,
  GitBranch,
  GitPullRequest,
  Globe,
  Library,
  ListTodo,
  MousePointerClick,
  PenTool,
  Plug,
  type LucideIcon,
} from "lucide-react";

/** MCP 传输类型，对齐标准 mcpServers 配置。 */
export type McpTransport = "stdio" | "http" | "sse";

export interface CatalogServer {
  id: string;
  name: string;
  /** 一句话用途的 i18n key(在消费端用 t() 解析,defaultValue 兜底中文原文)。 */
  purpose: string;
  transport: McpTransport;
  /** stdio 启动命令。 */
  command?: string;
  args?: string[];
  /** 必填 env 变量**名**（密钥）。安装时按命名输入收集，不内联明文。 */
  env?: string[];
  /** 远程 endpoint（http）。 */
  url?: string;
  /** 远程项是否走 OAuth（连接时浏览器授权，无静态密钥）。v1 仅存不连。 */
  oauth?: boolean;
  docsUrl: string;
  official: boolean;
  icon: LucideIcon;
}

/** 兜底图标（自定义 server / 没专属图标时）。 */
export const FALLBACK_ICON: LucideIcon = Plug;

export const MCP_CATALOG: CatalogServer[] = [
  {
    id: "filesystem",
    name: "Filesystem",
    purpose: "mcp.catalogPurpose.filesystem",
    transport: "stdio",
    command: "npx",
    args: ["-y", "@modelcontextprotocol/server-filesystem", "<目录绝对路径>"],
    docsUrl:
      "https://github.com/modelcontextprotocol/servers/tree/main/src/filesystem",
    official: true,
    icon: FolderTree,
  },
  {
    id: "git",
    name: "Git",
    purpose: "mcp.catalogPurpose.git",
    transport: "stdio",
    command: "uvx",
    args: ["mcp-server-git", "--repository", "<仓库绝对路径>"],
    docsUrl:
      "https://github.com/modelcontextprotocol/servers/tree/main/src/git",
    official: true,
    icon: GitBranch,
  },
  {
    id: "github",
    name: "GitHub",
    purpose: "mcp.catalogPurpose.github",
    transport: "http",
    url: "https://api.githubcopilot.com/mcp",
    env: ["GITHUB_PERSONAL_ACCESS_TOKEN"],
    docsUrl: "https://github.com/github/github-mcp-server",
    official: true,
    icon: GitPullRequest,
  },
  {
    id: "context7",
    name: "Context7",
    purpose: "mcp.catalogPurpose.context7",
    transport: "stdio",
    command: "npx",
    args: ["-y", "@upstash/context7-mcp"],
    env: ["CONTEXT7_API_KEY"],
    docsUrl: "https://github.com/upstash/context7",
    official: false,
    icon: Library,
  },
  {
    id: "playwright",
    name: "Playwright",
    purpose: "mcp.catalogPurpose.playwright",
    transport: "stdio",
    command: "npx",
    args: ["-y", "@playwright/mcp@latest"],
    docsUrl: "https://github.com/microsoft/playwright-mcp",
    official: true,
    icon: MousePointerClick,
  },
  {
    id: "chrome-devtools",
    name: "Chrome DevTools",
    purpose: "mcp.catalogPurpose.chrome-devtools",
    transport: "stdio",
    command: "npx",
    args: ["-y", "chrome-devtools-mcp@latest"],
    docsUrl: "https://github.com/ChromeDevTools/chrome-devtools-mcp",
    official: true,
    icon: Gauge,
  },
  {
    id: "fetch",
    name: "Fetch",
    purpose: "mcp.catalogPurpose.fetch",
    transport: "stdio",
    command: "uvx",
    args: ["mcp-server-fetch"],
    docsUrl:
      "https://github.com/modelcontextprotocol/servers/tree/main/src/fetch",
    official: true,
    icon: Globe,
  },
  {
    id: "postgres",
    name: "Postgres MCP Pro",
    purpose: "mcp.catalogPurpose.postgres",
    transport: "stdio",
    command: "postgres-mcp",
    args: [],
    env: ["DATABASE_URI"],
    docsUrl: "https://github.com/crystaldba/postgres-mcp",
    official: false,
    icon: Database,
  },
  {
    id: "sentry",
    name: "Sentry",
    purpose: "mcp.catalogPurpose.sentry",
    transport: "http",
    url: "https://mcp.sentry.dev/mcp",
    oauth: true,
    docsUrl: "https://docs.sentry.io/product/sentry-mcp/",
    official: true,
    icon: Bug,
  },
  {
    id: "linear",
    name: "Linear",
    purpose: "mcp.catalogPurpose.linear",
    transport: "http",
    url: "https://mcp.linear.app/mcp",
    oauth: true,
    docsUrl: "https://linear.app/docs/mcp",
    official: true,
    icon: ListTodo,
  },
  {
    id: "stripe",
    name: "Stripe",
    purpose: "mcp.catalogPurpose.stripe",
    transport: "http",
    url: "https://mcp.stripe.com",
    oauth: true,
    docsUrl: "https://docs.stripe.com/mcp",
    official: true,
    icon: CreditCard,
  },
  {
    id: "figma",
    name: "Figma",
    purpose: "mcp.catalogPurpose.figma",
    transport: "http",
    url: "https://mcp.figma.com/mcp",
    oauth: true,
    docsUrl: "https://help.figma.com/hc/en-us/articles/32132100833559",
    official: true,
    icon: PenTool,
  },
  {
    id: "brave-search",
    name: "Brave Search",
    purpose: "mcp.catalogPurpose.brave-search",
    transport: "stdio",
    command: "npx",
    args: ["-y", "@brave/brave-search-mcp-server"],
    env: ["BRAVE_API_KEY"],
    docsUrl: "https://github.com/brave/brave-search-mcp-server",
    official: true,
    icon: Compass,
  },
  {
    id: "sequential-thinking",
    name: "Sequential Thinking",
    purpose: "mcp.catalogPurpose.sequential-thinking",
    transport: "stdio",
    command: "npx",
    args: ["-y", "@modelcontextprotocol/server-sequential-thinking"],
    docsUrl:
      "https://github.com/modelcontextprotocol/servers/tree/main/src/sequentialthinking",
    official: true,
    icon: Brain,
  },
  {
    id: "aws",
    name: "AWS (awslabs)",
    purpose: "mcp.catalogPurpose.aws",
    transport: "stdio",
    command: "uvx",
    args: ["awslabs.aws-api-mcp-server@latest"],
    env: ["AWS_REGION"],
    docsUrl: "https://github.com/awslabs/mcp",
    official: true,
    icon: Cloud,
  },
];

/** args 里 `<...>` 形式的占位符（如 `<目录绝对路径>`）= 安装时需用户填的路径。 */
const PLACEHOLDER_ARG = /^<.+>$/;

/** 该 catalog 项是否需要先收集配置（密钥 env 或占位路径）再添加。
 *  无需配置的（如 fetch / sequential-thinking）支持一键直接装。 */
export function catalogNeedsConfig(s: CatalogServer): boolean {
  if (s.env && s.env.length > 0) return true;
  if (s.oauth) return false; // OAuth 连接时再授权，v1 仅存，无需先填
  return !!s.args?.some((a) => PLACEHOLDER_ARG.test(a));
}

export function isPlaceholderArg(arg: string): boolean {
  return PLACEHOLDER_ARG.test(arg);
}
