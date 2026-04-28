import { useEffect, useState } from "react";

export function ConnectionStatus() {
  const [online, setOnline] = useState(
    typeof navigator !== "undefined" ? navigator.onLine : true,
  );
  useEffect(() => {
    const up = () => setOnline(true);
    const down = () => setOnline(false);
    window.addEventListener("online", up);
    window.addEventListener("offline", down);
    return () => {
      window.removeEventListener("online", up);
      window.removeEventListener("offline", down);
    };
  }, []);
  return (
    <span
      className={`inline-block h-2 w-2 rounded-full ${online ? "bg-green-500" : "bg-red-500"}`}
      title={online ? "Online" : "Offline"}
    />
  );
}
