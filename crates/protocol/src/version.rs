use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{DeviceId, ProtocolError};

/// 장치별 수정 횟수 정보
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionVector(BTreeMap<DeviceId, u64>);

/// 두 벡터를 비교한 결과. 모두 "내 쪽(local) 기준"이다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Causality {
    /// 같은 이력이다.
    Equal,
    /// 내 벡터가 상대의 이력을 모두 포함하고 더 앞선다.
    LocalAhead,
    /// 상대 벡터가 내 이력을 모두 포함하고 더 앞선다.
    RemoteAhead,
    /// 서로 모르는 수정이 있다. 동시 수정이다.
    Concurrent,
}

impl VersionVector {
    /// 장치의 수정 횟수. 칸이 없으면 0이다.
    fn get(&self, device: &DeviceId) -> u64 {
        self.0.get(device).copied().unwrap_or(0)
    }

    /// 내 벡터를 상대 벡터와 칸별로 비교한다.
    pub fn compare(&self, remote: &VersionVector) -> Causality {
        let mut local_ahead = false;
        let mut remote_ahead = false;

        for device in self.0.keys().chain(remote.0.keys()) {
            let (mine, theirs) = (self.get(device), remote.get(device));
            local_ahead |= mine > theirs;
            remote_ahead |= theirs > mine;
        }

        match (local_ahead, remote_ahead) {
            (false, false) => Causality::Equal,
            (true, false) => Causality::LocalAhead,
            (false, true) => Causality::RemoteAhead,
            (true, true) => Causality::Concurrent,
        }
    }

    /// 이 장치가 파일을 한 번 고쳤다고 기록한다. 그 장치의 칸만 1 늘린다.
    pub fn bump(&mut self, device: DeviceId) {
        *self.0.entry(device).or_insert(0) += 1;
    }

    /// 상대의 이력을 합친다. 칸마다 둘 중 큰 값을 가진다.
    pub fn merge(&mut self, other: &VersionVector) {
        for (&device, &count) in &other.0 {
            let mine = self.0.entry(device).or_insert(0);
            *mine = (*mine).max(count);
        }
    }

    /// 저장소에 넣을 바이트로 바꾼다.
    pub fn to_bytes(&self) -> Result<Vec<u8>, ProtocolError> {
        Ok(postcard::to_stdvec(self)?)
    }

    /// 저장소에서 읽은 바이트를 벡터로 되돌린다.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ProtocolError> {
        Ok(postcard::from_bytes(bytes)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(name: &str) -> DeviceId {
        DeviceId::from_cert(name.as_bytes())
    }

    /// `[("a", 2), ("b", 1)]`처럼 장치 이름과 횟수로 벡터를 만든다.
    fn vector(counts: &[(&str, u64)]) -> VersionVector {
        VersionVector(
            counts
                .iter()
                .map(|&(name, count)| (device(name), count))
                .collect(),
        )
    }

    #[test]
    fn 같은_벡터는_equal이다() {
        let a = vector(&[("a", 2), ("b", 1)]);

        assert_eq!(a.compare(&a.clone()), Causality::Equal);
    }

    #[test]
    fn 둘_다_비어_있으면_equal이다() {
        assert_eq!(
            VersionVector::default().compare(&VersionVector::default()),
            Causality::Equal
        );
    }

    #[test]
    fn 상대의_칸이_모두_같거나_크면_remote_ahead다() {
        let local = vector(&[("a", 2), ("b", 1)]);
        let remote = vector(&[("a", 3), ("b", 1)]);

        assert_eq!(local.compare(&remote), Causality::RemoteAhead);
    }

    #[test]
    fn 내_칸이_모두_같거나_크면_local_ahead다() {
        let local = vector(&[("a", 3), ("b", 1)]);
        let remote = vector(&[("a", 2), ("b", 1)]);

        assert_eq!(local.compare(&remote), Causality::LocalAhead);
    }

    #[test]
    fn 서로_다른_칸에서_앞서면_concurrent다() {
        let local = vector(&[("a", 3), ("b", 1)]);
        let remote = vector(&[("a", 2), ("b", 2)]);

        assert_eq!(local.compare(&remote), Causality::Concurrent);
    }

    #[test]
    fn 한쪽에만_있는_칸은_0으로_본다() {
        let local = vector(&[("a", 1)]);
        let remote = vector(&[("a", 1), ("b", 1)]);

        assert_eq!(local.compare(&remote), Causality::RemoteAhead);
        assert_eq!(remote.compare(&local), Causality::LocalAhead);
    }

    #[test]
    fn 서로_모르는_장치에서_각자_고치면_concurrent다() {
        let local = vector(&[("a", 1)]);
        let remote = vector(&[("b", 1)]);

        assert_eq!(local.compare(&remote), Causality::Concurrent);
    }

    #[test]
    fn 값이_0인_칸은_없는_칸과_같다() {
        let local = vector(&[("a", 1), ("b", 0)]);
        let remote = vector(&[("a", 1)]);

        assert_eq!(local.compare(&remote), Causality::Equal);
    }

    #[test]
    fn bump는_그_장치의_칸만_1_늘린다() {
        let mut local = vector(&[("a", 1)]);

        local.bump(device("a"));
        local.bump(device("b"));

        assert_eq!(local, vector(&[("a", 2), ("b", 1)]));
    }

    #[test]
    fn 고치고_나면_이전_벡터보다_앞선다() {
        let before = vector(&[("a", 1)]);
        let mut after = before.clone();

        after.bump(device("a"));

        assert_eq!(after.compare(&before), Causality::LocalAhead);
    }

    #[test]
    fn merge는_칸마다_큰_값을_가진다() {
        let mut local = vector(&[("a", 3), ("b", 1)]);
        let remote = vector(&[("a", 2), ("b", 2), ("c", 1)]);

        local.merge(&remote);

        assert_eq!(local, vector(&[("a", 3), ("b", 2), ("c", 1)]));
    }

    #[test]
    fn 동시_수정을_합치고_고치면_양쪽보다_앞선다() {
        let mut local = vector(&[("a", 3), ("b", 1)]);
        let remote = vector(&[("a", 2), ("b", 2)]);

        local.merge(&remote);
        local.bump(device("a"));

        assert_eq!(local.compare(&remote), Causality::LocalAhead);
        assert_eq!(remote.compare(&local), Causality::RemoteAhead);
    }

    #[test]
    fn 바이트로_바꿨다_되돌리면_같은_벡터다() {
        let original = vector(&[("a", 2), ("b", 1)]);

        let bytes = original.to_bytes().unwrap();

        assert_eq!(VersionVector::from_bytes(&bytes).unwrap(), original);
    }

    #[test]
    fn 빈_벡터도_바이트로_왕복한다() {
        let bytes = VersionVector::default().to_bytes().unwrap();

        assert!(!bytes.is_empty());
        assert_eq!(
            VersionVector::from_bytes(&bytes).unwrap(),
            VersionVector::default()
        );
    }

    #[test]
    fn 빈_바이트는_벡터로_읽을_수_없다() {
        assert!(VersionVector::from_bytes(&[]).is_err());
    }
}
