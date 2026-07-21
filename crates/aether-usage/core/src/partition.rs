pub fn partition_for_subject(subject_id: &str, partitions: u16) -> u16 {
    if partitions == 0 {
        return 0;
    }
    let hash = subject_id.bytes().fold(0u32, |acc, byte| {
        acc.wrapping_mul(16777619).wrapping_add(u32::from(byte))
    });
    (hash % u32::from(partitions)) as u16
}

#[cfg(test)]
mod tests {
    use super::partition_for_subject;

    #[test]
    fn subject_partition_is_stable() {
        assert_eq!(
            partition_for_subject("user-1", 16),
            partition_for_subject("user-1", 16)
        );
    }
}
