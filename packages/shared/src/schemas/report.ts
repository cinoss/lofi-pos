import { z } from "zod";

/// Plan F: a single `daily_report` row as returned by GET /admin/reports/:day.
/// `order_summary_json` is the raw stringified `Report` payload — callers
/// `JSON.parse` it on demand.
export const DailyReport = z
  .object({
    business_day: z.string(),
    generated_at: z.number().int(),
    order_summary_json: z.string(),
    inventory_summary_json: z.string(),
  })
  .strict();
export type DailyReport = z.infer<typeof DailyReport>;

export const DailyReportList = z.array(DailyReport);
export type DailyReportList = z.infer<typeof DailyReportList>;
