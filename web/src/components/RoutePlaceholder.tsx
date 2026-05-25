/**
 * Placeholder for routes still in the implementation queue. Lets the
 * AppShell nav exercise every path before each route's real surface
 * lands, so 404s don't mask broken router config.
 */
export function RoutePlaceholder({ name, pencilId }: { name: string; pencilId?: string }) {
  return (
    <div className="flex h-full flex-col items-center justify-center gap-3 bg-surface-primary p-10 text-center">
      <h1 className="font-heading text-2xl font-semibold text-foreground-primary">{name}</h1>
      <p className="font-caption text-sm text-foreground-tertiary">
        待铺设计稿{pencilId ? ` · Pencil id: ${pencilId}` : ""}
      </p>
    </div>
  );
}
