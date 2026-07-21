ALTER TABLE IF EXISTS public.announcement_reads
    DROP CONSTRAINT IF EXISTS announcement_reads_announcement_id_fkey;

ALTER TABLE IF EXISTS public.announcement_reads
    ADD CONSTRAINT announcement_reads_announcement_id_fkey
    FOREIGN KEY (announcement_id)
    REFERENCES public.announcements(id)
    ON DELETE CASCADE;
