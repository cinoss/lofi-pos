import type { ReactNode } from "react";
import { Button } from "@lofi-pos/ui/components/button";

interface Props {
  open: boolean;
  title: string;
  onClose: () => void;
  children: ReactNode;
}

/// Plain centered modal. Mirrors the inline style in pos-ui's
/// `OverrideModal` so we don't depend on a shadcn Dialog primitive
/// (`@lofi-pos/ui` ships only a Button today).
export function Modal({ open, title, onClose, children }: Props) {
  if (!open) return null;
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50">
      <div className="w-full max-w-md rounded-lg bg-white p-6 shadow-xl">
        <div className="mb-4 flex items-center justify-between">
          <h2 className="text-lg font-semibold">{title}</h2>
          <Button size="sm" variant="ghost" onClick={onClose}>
            Close
          </Button>
        </div>
        {children}
      </div>
    </div>
  );
}
