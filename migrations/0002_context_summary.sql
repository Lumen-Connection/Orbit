ALTER TABLE session ADD COLUMN context_summary TEXT;
ALTER TABLE session ADD COLUMN context_summary_upto INTEGER NOT NULL DEFAULT 0;
ALTER TABLE usage_record ADD COLUMN kind TEXT NOT NULL DEFAULT 'turn';
