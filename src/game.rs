//! Game rules and state. Pure logic: time is passed in (ms), so everything here is unit-tested.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};

pub const REVEAL_MS: u64 = 7000;
pub const COUNTDOWN_MS: u64 = 3500;
pub const MAX_PER_Q: u32 = 1000;
/// Answers arriving this long after the deadline still count (network delay).
pub const GRACE_MS: u64 = 400;
/// The question closes this long after the deadline.
pub const CLOSE_DELAY_MS: u64 = 600;
/// When every online team has answered, the question closes after this delay.
pub const ALL_IN_DELAY_MS: u64 = 700;
/// A phone counts as online this long after its last poll ended (it re-polls right away).
pub const POLL_GRACE_MS: u64 = 5000;

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct Question {
    pub q: String,
    pub options: Vec<String>,
    pub answer: usize,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct Part {
    pub title: String,
    pub questions: Vec<Question>,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct Quiz {
    #[serde(default = "default_title")]
    pub title: String,
    #[serde(default)]
    pub subtitle: String,
    #[serde(default = "default_seconds")]
    pub question_seconds: u64,
    #[serde(default = "default_teams")]
    pub teams: Vec<u32>,
    pub parts: Vec<Part>,
}
fn default_title() -> String {
    "Quiz Race".into()
}
fn default_seconds() -> u64 {
    20
}
fn default_teams() -> Vec<u32> {
    (1..=9).collect()
}

pub const BUILTIN_QUIZ: &str = include_str!("../assets/questions.json");

impl Quiz {
    pub fn builtin() -> Quiz {
        Quiz::parse(BUILTIN_QUIZ).expect("built-in questions.json is valid")
    }

    pub fn parse(s: &str) -> Result<Quiz, String> {
        let q: Quiz = serde_json::from_str(s).map_err(|e| e.to_string())?;
        q.validate()?;
        Ok(q)
    }

    fn validate(&self) -> Result<(), String> {
        if self.parts.is_empty() {
            return Err("no parts".into());
        }
        if !(5..=300).contains(&self.question_seconds) {
            return Err("question_seconds must be 5–300".into());
        }
        if self.teams.is_empty() || self.teams.len() > 12 {
            return Err("teams: 1–12 groups".into());
        }
        let mut seen = self.teams.clone();
        seen.sort();
        seen.dedup();
        if seen.len() != self.teams.len() || seen.iter().any(|&t| t == 0 || t > 99) {
            return Err("teams: unique numbers 1–99".into());
        }
        for (pi, p) in self.parts.iter().enumerate() {
            if p.questions.is_empty() {
                return Err(format!("part {} has no questions", pi + 1));
            }
            for (qi, q) in p.questions.iter().enumerate() {
                let n = q.options.len();
                if !(2..=4).contains(&n) || q.answer >= n {
                    return Err(format!("part {} question {}: need 2–4 options and a valid answer index", pi + 1, qi + 1));
                }
            }
        }
        Ok(())
    }

    pub fn total_questions(&self) -> usize {
        self.parts.iter().map(|p| p.questions.len()).sum()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Waiting,
    Countdown,
    Question,
    Reveal,
    Results,
    Final,
}

#[derive(Clone, Debug)]
pub struct Team {
    pub id: u32,
    pub avatar: String,
    pub token: String,
    pub scores: Vec<u32>,
    pub correct: Vec<u32>,
    pub time: Vec<u64>,
}

impl Team {
    pub fn total(&self) -> u32 {
        self.scores.iter().sum()
    }
}

#[derive(Clone, Copy, Debug)]
struct Answer {
    choice: usize,
    ms: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct TeamResult {
    pub choice: Option<usize>,
    pub correct: bool,
    pub points: u32,
    pub ms: u64,
}

#[derive(Clone, Debug)]
pub struct Reveal {
    pub correct: usize,
    pub counts: Vec<u32>,
    pub results: BTreeMap<u32, TeamResult>,
    /// The question closed because time ran out (not skipped / everyone answered).
    pub timed_out: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Ranked {
    pub id: u32,
    pub rank: usize,
    pub pts: u32,
    pub cor: u32,
    pub ms: u64,
}

pub struct Game {
    pub quiz: Quiz,
    pub phase: Phase,
    /// 0-based part index.
    pub part: usize,
    /// 0-based question index within the part.
    pub qi: usize,
    pub q_start: u64,
    pub q_end: u64,
    pub cd_end: u64,
    close_at: u64,
    reveal_at: u64,
    pub auto: bool,
    pub teams: BTreeMap<u32, Team>,
    answers: HashMap<u32, Answer>,
    pub reveal: Option<Reveal>,
    /// Long-poll requests per device token: how many are open, and when the last one ended.
    conns: HashMap<String, (u32, u64)>,
    /// Time of the last tick.
    clock: u64,
    /// Bumped on every change; event streams wait on it.
    pub version: u64,
}

pub fn valid_token(t: &str) -> bool {
    (8..=64).contains(&t.len()) && t.bytes().all(|b| b.is_ascii_alphanumeric())
}

impl Game {
    pub fn new(quiz: Quiz) -> Game {
        Game {
            quiz,
            phase: Phase::Waiting,
            part: 0,
            qi: 0,
            q_start: 0,
            q_end: 0,
            cd_end: 0,
            close_at: 0,
            reveal_at: 0,
            auto: true,
            teams: BTreeMap::new(),
            answers: HashMap::new(),
            reveal: None,
            conns: HashMap::new(),
            clock: 0,
            version: 1,
        }
    }

    fn q_ms(&self) -> u64 {
        self.quiz.question_seconds * 1000
    }
    pub fn q_count(&self) -> usize {
        self.quiz.parts[self.part].questions.len()
    }
    pub fn question(&self) -> &Question {
        &self.quiz.parts[self.part].questions[self.qi]
    }
    pub fn max_per_part(&self, part: usize) -> u32 {
        MAX_PER_Q * self.quiz.parts[part].questions.len() as u32
    }
    pub fn online(&self, id: u32) -> bool {
        self.teams.get(&id).is_some_and(|t| self.token_online(&t.token))
    }
    fn token_online(&self, token: &str) -> bool {
        self.conns.get(token).is_some_and(|&(open, last)| open > 0 || self.clock < last + POLL_GRACE_MS)
    }
    pub fn answered(&self, id: u32) -> bool {
        self.answers.contains_key(&id)
    }
    pub fn team_by_token(&self, token: &str) -> Option<&Team> {
        self.teams.values().find(|t| t.token == token)
    }
    fn bump(&mut self) {
        self.version += 1;
    }

    // ---------- flow ----------

    pub fn start(&mut self, now: u64) {
        if self.phase == Phase::Waiting {
            self.phase = Phase::Countdown;
            self.cd_end = now + COUNTDOWN_MS;
            self.reveal = None;
            self.bump();
        }
    }

    /// Run timers. Call often (every ~50 ms).
    pub fn tick(&mut self, now: u64) {
        self.clock = now;
        // phones that stopped polling go offline
        let gone: Vec<String> =
            self.conns.iter().filter(|(_, &(open, last))| open == 0 && now >= last + POLL_GRACE_MS).map(|(k, _)| k.clone()).collect();
        for k in gone {
            self.conns.remove(&k);
            if self.team_by_token(&k).is_some() {
                self.check_all_in(now);
                self.bump();
            }
        }
        match self.phase {
            Phase::Countdown if now >= self.cd_end => self.send_question(0, now),
            Phase::Question if now >= self.close_at => self.end_question(now, now >= self.q_end + CLOSE_DELAY_MS),
            Phase::Reveal if self.auto && now >= self.reveal_at + REVEAL_MS => self.advance(now),
            _ => {}
        }
    }

    fn send_question(&mut self, i: usize, now: u64) {
        self.phase = Phase::Question;
        self.qi = i;
        self.answers.clear();
        self.reveal = None;
        self.q_start = now;
        self.q_end = now + self.q_ms();
        self.close_at = self.q_end + CLOSE_DELAY_MS;
        self.bump();
    }

    fn end_question(&mut self, now: u64, timed_out: bool) {
        if self.phase != Phase::Question {
            return;
        }
        let q_ms = self.q_ms();
        let pi = self.part;
        let (correct_idx, n_opts) = {
            let q = self.question();
            (q.answer, q.options.len())
        };
        let mut counts = vec![0u32; n_opts];
        let mut results = BTreeMap::new();
        for t in self.teams.values_mut() {
            let a = self.answers.get(&t.id).copied();
            let (mut points, mut correct, mut ms) = (0, false, q_ms);
            if let Some(a) = a {
                ms = a.ms;
                counts[a.choice] += 1;
                correct = a.choice == correct_idx;
                if correct {
                    points = score(ms, q_ms);
                }
            }
            t.scores[pi] += points;
            t.correct[pi] += correct as u32;
            t.time[pi] += ms;
            results.insert(t.id, TeamResult { choice: a.map(|a| a.choice), correct, points, ms });
        }
        self.phase = Phase::Reveal;
        self.reveal_at = now;
        self.reveal = Some(Reveal { correct: correct_idx, counts, results, timed_out });
        self.bump();
    }

    fn advance(&mut self, now: u64) {
        if self.phase == Phase::Reveal {
            if self.qi + 1 < self.q_count() {
                self.send_question(self.qi + 1, now);
            } else {
                self.phase = Phase::Results;
                self.bump();
            }
        }
    }

    /// Host: end the current question now.
    pub fn skip(&mut self, now: u64) {
        self.end_question(now, false);
    }

    /// Host: the main "next" button for reveal / results.
    pub fn next(&mut self, now: u64) {
        match self.phase {
            Phase::Reveal => self.advance(now),
            Phase::Results => {
                if self.part + 1 < self.quiz.parts.len() {
                    self.part += 1;
                    self.phase = Phase::Waiting;
                    self.qi = 0;
                    self.reveal = None;
                } else {
                    self.phase = Phase::Final;
                }
                self.bump();
            }
            _ => {}
        }
    }

    pub fn set_auto(&mut self, v: bool, now: u64) {
        self.auto = v;
        if self.phase == Phase::Reveal {
            self.reveal_at = now; // full reveal time again, like the original
        }
        self.bump();
    }

    pub fn kick(&mut self, id: u32, now: u64) {
        if self.teams.remove(&id).is_some() {
            self.answers.remove(&id);
            self.check_all_in(now);
            self.bump();
        }
    }

    /// Host: clear all scores, keep joined teams. `quiz` replaces the questions (reloaded file).
    pub fn reset(&mut self, quiz: Option<Quiz>) {
        if let Some(q) = quiz {
            self.quiz = q;
        }
        let n = self.quiz.parts.len();
        let ids = self.quiz.teams.clone();
        let old = std::mem::take(&mut self.teams);
        let conns = std::mem::take(&mut self.conns);
        let version = self.version + 1; // keep counting up so waiting polls see the change
        *self = Game { auto: self.auto, version, clock: self.clock, ..Game::new(self.quiz.clone()) };
        self.conns = conns;
        for (id, t) in old.into_iter().filter(|(id, _)| ids.contains(id)) {
            self.teams.insert(id, Team { scores: vec![0; n], correct: vec![0; n], time: vec![0; n], ..t });
        }
    }

    // ---------- players ----------

    /// A poll from this phone started. Only a change offline → online counts as a change
    /// (otherwise every poll would wake every other poll).
    pub fn connect(&mut self, token: &str) {
        let was = self.token_online(token);
        self.conns.entry(token.to_string()).or_insert((0, 0)).0 += 1;
        if !was && self.team_by_token(token).is_some() {
            self.bump();
        }
    }

    /// A poll ended; the phone stays online for POLL_GRACE_MS (see `tick`).
    pub fn disconnect(&mut self, token: &str, now: u64) {
        if let Some(c) = self.conns.get_mut(token) {
            c.0 = c.0.saturating_sub(1);
            c.1 = now;
        }
    }

    pub fn join(&mut self, token: &str, team: u32, avatar: &str) -> Result<(), String> {
        if !valid_token(token) || !self.quiz.teams.contains(&team) || !crate::emoji_list::AVATARS.contains(&avatar) {
            return Err("Invalid choice.".into());
        }
        if let Some(ex) = self.teams.get(&team) {
            if ex.token != token && self.online(team) {
                return Err(format!("Group {team} is already taken by another device."));
            }
        }
        // a device holds one team: release the one it had before
        let mine: Vec<u32> = self.teams.values().filter(|t| t.token == token && t.id != team).map(|t| t.id).collect();
        for id in mine {
            self.teams.remove(&id);
        }
        let n = self.quiz.parts.len();
        let t = self.teams.entry(team).or_insert_with(|| Team {
            id: team,
            avatar: String::new(),
            token: String::new(),
            scores: vec![0; n],
            correct: vec![0; n],
            time: vec![0; n],
        });
        t.token = token.to_string();
        t.avatar = avatar.to_string();
        self.bump();
        Ok(())
    }

    /// Leave the team (only while waiting, so nobody loses points by accident mid-part).
    pub fn leave(&mut self, token: &str) {
        if self.phase != Phase::Waiting {
            return;
        }
        if let Some(id) = self.team_by_token(token).map(|t| t.id) {
            self.teams.remove(&id);
            self.bump();
        }
    }

    pub fn answer(&mut self, token: &str, choice: usize, now: u64) -> bool {
        if self.phase != Phase::Question || choice >= self.question().options.len() || now > self.q_end + GRACE_MS {
            return false;
        }
        let Some(id) = self.team_by_token(token).map(|t| t.id) else { return false };
        if self.answers.contains_key(&id) {
            return false;
        }
        let ms = now.saturating_sub(self.q_start).min(self.q_ms());
        self.answers.insert(id, Answer { choice, ms });
        self.check_all_in(now);
        self.bump();
        true
    }

    fn check_all_in(&mut self, now: u64) {
        if self.phase != Phase::Question {
            return;
        }
        let active: Vec<u32> = self.teams.keys().copied().filter(|&id| self.online(id)).collect();
        if !active.is_empty() && active.iter().all(|id| self.answers.contains_key(id)) {
            self.close_at = self.close_at.min(now + ALL_IN_DELAY_MS);
        }
    }

    // ---------- views ----------

    /// Rank by points (desc), correct answers (desc), total time (asc), then group number.
    /// `part = None` ranks the whole game.
    pub fn ranking(&self, part: Option<usize>) -> Vec<Ranked> {
        let mut v: Vec<Ranked> = self
            .teams
            .values()
            .map(|t| {
                let (pts, cor, ms) = match part {
                    Some(p) => (t.scores[p], t.correct[p], t.time[p]),
                    None => (t.total(), t.correct.iter().sum(), t.time.iter().sum()),
                };
                Ranked { id: t.id, rank: 0, pts, cor, ms }
            })
            .collect();
        v.sort_by(|a, b| b.pts.cmp(&a.pts).then(b.cor.cmp(&a.cor)).then(a.ms.cmp(&b.ms)).then(a.id.cmp(&b.id)));
        for (i, r) in v.iter_mut().enumerate() {
            r.rank = i + 1;
        }
        v
    }

    /// State sent to one phone.
    pub fn player_view(&self, token: &str, now: u64) -> Value {
        let me = self.team_by_token(token);
        let teams: Vec<Value> = self
            .teams
            .values()
            .map(|t| {
                json!({ "id": t.id, "avatar": t.avatar, "connected": self.online(t.id),
                        "scores": t.scores, "correct": t.correct, "time": t.time, "total": t.total() })
            })
            .collect();
        let mut v = json!({
            "phase": self.phase, "part": self.part, "qi": self.qi, "qCount": self.q_count(),
            "qSeconds": self.quiz.question_seconds, "title": self.quiz.title, "subtitle": self.quiz.subtitle,
            "partTitles": self.quiz.parts.iter().map(|p| p.title.as_str()).collect::<Vec<_>>(),
            "partCount": self.quiz.parts.len(), "totalQ": self.quiz.total_questions(),
            "teamIds": self.quiz.teams, "avatars": crate::emoji_list::AVATARS,
            "maxPerPart": self.max_per_part(self.part), "teams": teams,
            "me": me.map(|t| json!({ "id": t.id, "avatar": t.avatar })),
            "myChoice": me.and_then(|t| self.answers.get(&t.id)).map(|a| a.choice),
        });
        if self.phase == Phase::Countdown {
            v["countdownMs"] = json!(self.cd_end.saturating_sub(now));
        }
        if matches!(self.phase, Phase::Question | Phase::Reveal) {
            let q = self.question();
            v["question"] = json!({ "q": q.q, "options": q.options });
            if self.phase == Phase::Question {
                v["remainingMs"] = json!(self.q_end.saturating_sub(now));
            }
        }
        if let Some(r) = &self.reveal {
            v["reveal"] = json!({ "correct": r.correct, "mine": me.and_then(|t| r.results.get(&t.id)) });
        }
        v
    }
}

/// Correct answer: 500–1000 points, faster = more.
pub fn score(ms: u64, q_ms: u64) -> u32 {
    let f = 1.0 - ms.min(q_ms) as f64 / q_ms as f64;
    (MAX_PER_Q as f64 * (0.5 + 0.5 * f)).round() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "tokenAAAA1";
    const B: &str = "tokenBBBB2";

    fn game() -> Game {
        let mut g = Game::new(Quiz::builtin());
        g.join(A, 1, "🦊").unwrap();
        g.join(B, 2, "🐼").unwrap();
        g.connect(A);
        g.connect(B);
        g
    }

    /// Start part and reach question 1 at t=10_000.
    fn to_question(g: &mut Game) -> u64 {
        g.start(6_500);
        g.tick(9_999);
        assert_eq!(g.phase, Phase::Countdown);
        g.tick(10_000);
        assert_eq!(g.phase, Phase::Question);
        10_000
    }

    #[test]
    fn builtin_quiz_matches_original() {
        let q = Quiz::builtin();
        assert_eq!(q.parts.len(), 3);
        assert_eq!(q.total_questions(), 15);
        assert_eq!(q.teams, vec![1, 2, 3, 4, 6, 7, 8, 9, 10]);
        assert_eq!(q.parts[0].questions[0].answer, 1);
    }

    #[test]
    fn scoring_formula() {
        assert_eq!(score(0, 20_000), 1000);
        assert_eq!(score(20_000, 20_000), 500);
        assert_eq!(score(10_000, 20_000), 750);
        assert_eq!(score(99_999, 20_000), 500);
    }

    #[test]
    fn rejects_bad_quiz_files() {
        assert!(Quiz::parse("{").is_err());
        assert!(Quiz::parse(r#"{"parts":[]}"#).is_err());
        assert!(Quiz::parse(r#"{"parts":[{"title":"x","questions":[{"q":"?","options":["a","b"],"answer":2}]}]}"#).is_err());
        assert!(Quiz::parse(r#"{"teams":[1,1],"parts":[{"title":"x","questions":[{"q":"?","options":["a","b"],"answer":1}]}]}"#).is_err());
        let ok = Quiz::parse(r#"{"parts":[{"title":"x","questions":[{"q":"?","options":["a","b"],"answer":1}]}]}"#).unwrap();
        assert_eq!(ok.question_seconds, 20);
        assert_eq!(ok.teams.len(), 9);
    }

    #[test]
    fn join_rules() {
        let mut g = game();
        assert!(g.join("tokenCCCC3", 5, "🦊").is_err(), "group 5 is not playing");
        assert!(g.join("tokenCCCC3", 3, "<style>").is_err(), "avatar must come from the list");
        assert!(g.join("bad token!", 3, "🦊").is_err());
        assert!(g.join("tokenCCCC3", 1, "🐸").unwrap_err().contains("already taken"));
        // the same device switching group releases its old one
        g.join(A, 3, "🐸").unwrap();
        assert!(!g.teams.contains_key(&1));
        assert_eq!(g.teams[&3].avatar, "🐸");
        // an offline team's slot can be taken over
        g.disconnect(B, 0);
        g.tick(POLL_GRACE_MS);
        g.join("tokenCCCC3", 2, "🐸").unwrap();
        assert_eq!(g.teams[&2].token, "tokenCCCC3");
    }

    #[test]
    fn full_question_flow_and_points() {
        let mut g = game();
        let t0 = to_question(&mut g);
        let right = g.question().answer;
        assert!(g.answer(B, (right + 1) % 4, t0 + 1_000));
        assert!(g.answer(A, right, t0 + 5_000));
        assert!(!g.answer(A, right, t0 + 5_000), "one answer per team");
        // everyone answered → closes after a short delay, not at the deadline
        g.tick(t0 + 5_000 + ALL_IN_DELAY_MS - 1);
        assert_eq!(g.phase, Phase::Question);
        g.tick(t0 + 5_000 + ALL_IN_DELAY_MS);
        assert_eq!(g.phase, Phase::Reveal);
        let r = g.reveal.as_ref().unwrap();
        assert!(!r.timed_out);
        assert_eq!(r.results[&1].points, 875);
        assert_eq!(r.results[&2].points, 0);
        assert_eq!(r.counts.iter().sum::<u32>(), 2);
        assert_eq!(g.teams[&1].scores[0], 875);
        assert_eq!(g.teams[&1].correct[0], 1);
        assert_eq!(g.teams[&2].time[0], 1_000);
        // the answer stays on screen for REVEAL_MS, then auto-advances
        let t1 = t0 + 5_700;
        g.tick(t1 + REVEAL_MS - 1);
        assert_eq!(g.phase, Phase::Reveal);
        g.tick(t1 + REVEAL_MS);
        assert_eq!((g.phase, g.qi), (Phase::Question, 1));
    }

    #[test]
    fn timeout_and_late_answers() {
        let mut g = game();
        let t0 = to_question(&mut g);
        let q_ms = 20_000;
        assert!(g.answer(A, 0, t0 + q_ms + GRACE_MS), "grace period");
        assert!(!g.answer(B, 0, t0 + q_ms + GRACE_MS + 1), "too late");
        g.tick(t0 + q_ms + CLOSE_DELAY_MS);
        assert_eq!(g.phase, Phase::Reveal);
        assert!(g.reveal.as_ref().unwrap().timed_out);
        assert_eq!(g.teams[&2].time[0], q_ms as u64, "no answer counts full time");
    }

    #[test]
    fn offline_teams_do_not_block_and_manual_mode_waits() {
        let mut g = game();
        g.disconnect(B, 0);
        g.tick(POLL_GRACE_MS);
        assert!(!g.online(2));
        g.set_auto(false, 0);
        let t0 = to_question(&mut g);
        g.answer(A, 0, t0 + 100);
        g.tick(t0 + 100 + ALL_IN_DELAY_MS);
        assert_eq!(g.phase, Phase::Reveal);
        g.tick(t0 + 999_999);
        assert_eq!(g.phase, Phase::Reveal, "auto off: host must press next");
        g.next(t0 + 999_999);
        assert_eq!(g.phase, Phase::Question);
    }

    #[test]
    fn whole_game_reaches_final_and_ranks() {
        let mut g = game();
        let mut now = 0;
        for part in 0..3 {
            assert_eq!((g.phase, g.part), (Phase::Waiting, part));
            g.start(now);
            now += COUNTDOWN_MS;
            g.tick(now);
            for _ in 0..5 {
                let right = g.question().answer;
                g.answer(A, right, now + 2_000);
                g.answer(B, right, now + 4_000);
                now += 2_000 + ALL_IN_DELAY_MS + 2_000;
                g.tick(now);
                assert_eq!(g.phase, Phase::Reveal);
                g.next(now);
            }
            assert_eq!(g.phase, Phase::Results);
            g.next(now);
        }
        assert_eq!(g.phase, Phase::Final);
        let r = g.ranking(None);
        assert_eq!((r[0].id, r[0].rank, r[0].cor), (1, 1, 15));
        assert_eq!(r[1].id, 2);
        assert!(r[0].pts > r[1].pts);
        assert_eq!(g.ranking(Some(2))[0].pts, 5 * 950);
    }

    #[test]
    fn ranking_tie_breaks() {
        let mut g = game();
        for t in g.teams.values_mut() {
            t.scores[0] = 1000;
            t.correct[0] = 1;
        }
        g.teams.get_mut(&1).unwrap().time[0] = 9_000;
        g.teams.get_mut(&2).unwrap().time[0] = 3_000;
        assert_eq!(g.ranking(Some(0)).iter().map(|r| r.id).collect::<Vec<_>>(), vec![2, 1]);
        g.teams.get_mut(&1).unwrap().correct[0] = 2;
        assert_eq!(g.ranking(Some(0))[0].id, 1);
    }

    #[test]
    fn reset_keeps_teams_clears_scores() {
        let mut g = game();
        let t0 = to_question(&mut g);
        g.answer(A, g.question().answer, t0);
        g.skip(t0);
        assert!(g.teams[&1].total() > 0);
        let v = g.version;
        g.reset(None);
        assert_eq!(g.phase, Phase::Waiting);
        assert_eq!(g.teams.len(), 2);
        assert_eq!(g.teams[&1].total(), 0);
        assert!(g.online(1), "connections survive a reset");
        assert_ne!(g.version, v);
    }

    #[test]
    fn player_view_hides_other_teams_answers() {
        let mut g = game();
        let t0 = to_question(&mut g);
        g.answer(A, 2, t0 + 10);
        let v = g.player_view(B, t0 + 10);
        assert_eq!(v["me"]["id"], 2);
        assert!(v["myChoice"].is_null());
        assert_eq!(v["question"]["options"].as_array().unwrap().len(), 4);
        assert!(v.get("reveal").is_none());
        assert_eq!(g.player_view(A, t0)["myChoice"], 2);
        assert!(g.player_view("nobodyXXXX", t0)["me"].is_null());
    }

    #[test]
    fn polling_keeps_phone_online_without_waking_everyone() {
        let mut g = game();
        let v = g.version;
        // poll ends, next poll starts a moment later: still online, no state change
        g.disconnect(A, 1_000);
        g.tick(1_050);
        assert!(g.online(1));
        g.connect(A);
        assert_eq!(g.version, v);
        // phone gone: offline after the grace period, and that is a change
        g.disconnect(A, 2_000);
        g.tick(2_000 + POLL_GRACE_MS - 1);
        assert!(g.online(1));
        g.tick(2_000 + POLL_GRACE_MS);
        assert!(!g.online(1));
        assert!(g.version > v);
        // back again → online, and that is a change too
        let v = g.version;
        g.connect(A);
        assert!(g.online(1) && g.version > v);
    }

    #[test]
    fn kick_and_leave() {
        let mut g = game();
        g.kick(2, 0);
        assert!(!g.teams.contains_key(&2));
        g.start(0);
        g.leave(A);
        assert!(g.teams.contains_key(&1), "cannot leave during a part");
    }
}
