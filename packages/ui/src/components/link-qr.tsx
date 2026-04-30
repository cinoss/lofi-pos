import { useEffect, useRef } from "react";
import QRCode from "qrcode";

interface LinkQRProps {
  url: string;
  /** Show URL text above the QR code; default true */
  showText?: boolean;
  /** QR pixel width; default 192 */
  size?: number;
  /** Optional label e.g. "Scan to open setup on your phone" */
  label?: string;
}

/**
 * Renders a URL as both a clickable link and a scannable QR code. Useful for
 * cross-device hand-off — a tablet cashier can show this and a phone on the
 * same Wi-Fi can scan to open the URL.
 */
export function LinkQR({ url, showText = true, size = 192, label }: LinkQRProps) {
  const canvas = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    if (canvas.current) {
      void QRCode.toCanvas(canvas.current, url, { width: size, margin: 1 });
    }
  }, [url, size]);
  return (
    <div className="flex flex-col items-center gap-2">
      {label && <div className="text-sm text-gray-600">{label}</div>}
      <a
        href={url}
        target="_blank"
        rel="noreferrer"
        className="text-blue-600 underline break-all text-center text-sm"
      >
        {showText ? url : "Open link"}
      </a>
      <canvas ref={canvas} width={size} height={size} />
    </div>
  );
}
