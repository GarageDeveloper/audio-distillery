import { useRef, type ReactNode } from "react";

interface Props {
  onClose: () => void;
  children: ReactNode;
}

/**
 * Modal backdrop that closes ONLY when a click both starts and ends on the
 * backdrop itself. A text-selection drag that leaves an input and releases
 * over the backdrop must not dismiss the dialog.
 */
export function Backdrop({ onClose, children }: Props) {
  const downOnBackdrop = useRef(false);
  return (
    <div
      className="modal-backdrop"
      onMouseDown={(e) => {
        downOnBackdrop.current = e.target === e.currentTarget;
      }}
      onClick={(e) => {
        if (e.target === e.currentTarget && downOnBackdrop.current) onClose();
        downOnBackdrop.current = false;
      }}
    >
      {children}
    </div>
  );
}
