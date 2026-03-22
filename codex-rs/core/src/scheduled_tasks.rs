use std::collections::BTreeMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use chrono::DateTime;
use chrono::Datelike;
use chrono::Duration as ChronoDuration;
use chrono::Local;
use chrono::LocalResult;
use chrono::TimeZone;
use chrono::Timelike;
use chrono::Utc;
use serde::Serialize;
use tokio::sync::Mutex;

const MAX_TASKS: usize = 50;
const RECURRING_LIFETIME_HOURS: i64 = 72;
const MAX_RECURRING_JITTER_SECONDS: i64 = 15 * 60;
const MAX_ONE_SHOT_EARLY_JITTER_SECONDS: i64 = 90;
const MAX_SEARCH_MINUTES: i64 = 60 * 24 * 366 * 5;

pub(crate) fn cron_tools_enabled(config: &crate::config::Config) -> bool {
    !config.disable_cron
}

#[derive(Clone, Debug)]
enum TaskSchedule {
    Cron(CronSchedule),
    Once { run_at: DateTime<Utc> },
}

#[derive(Clone, Debug)]
struct ScheduledTask {
    id: String,
    prompt: String,
    created_at: DateTime<Utc>,
    expires_at: Option<DateTime<Utc>>,
    next_run_at: DateTime<Utc>,
    schedule: TaskSchedule,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DueTask {
    pub(crate) id: String,
    pub(crate) prompt: String,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ScheduledTaskKind {
    Cron,
    Once,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub(crate) struct ScheduledTaskInfo {
    pub(crate) id: String,
    pub(crate) kind: ScheduledTaskKind,
    pub(crate) prompt: String,
    pub(crate) schedule: Option<String>,
    pub(crate) run_at: Option<DateTime<Utc>>,
    pub(crate) next_run_at: DateTime<Utc>,
    pub(crate) created_at: DateTime<Utc>,
}

impl ScheduledTaskInfo {
    fn from_task(task: &ScheduledTask) -> Self {
        let (kind, schedule, run_at) = match &task.schedule {
            TaskSchedule::Cron(schedule) => (
                ScheduledTaskKind::Cron,
                Some(schedule.original.clone()),
                None,
            ),
            TaskSchedule::Once { run_at } => (ScheduledTaskKind::Once, None, Some(*run_at)),
        };

        Self {
            id: task.id.clone(),
            kind,
            prompt: task.prompt.clone(),
            schedule,
            run_at,
            next_run_at: task.next_run_at,
            created_at: task.created_at,
        }
    }
}

#[derive(Default)]
pub(crate) struct ScheduledTasks {
    next_id: AtomicU64,
    tasks: Mutex<BTreeMap<String, ScheduledTask>>,
}

impl ScheduledTasks {
    pub(crate) async fn create_cron(
        &self,
        expression: &str,
        prompt: &str,
        now: DateTime<Utc>,
    ) -> Result<ScheduledTaskInfo, String> {
        let prompt = normalize_prompt(prompt)?;
        let schedule = CronSchedule::parse(expression)?;
        let id = self.next_task_id();
        let expires_at = Some(now + ChronoDuration::hours(RECURRING_LIFETIME_HOURS));
        let Some(next_run_at) = schedule.next_run_at(&id, now, expires_at) else {
            return Err(
                "schedule does not trigger within the 3-day recurring task lifetime".to_string(),
            );
        };

        let task = ScheduledTask {
            id,
            prompt,
            created_at: now,
            expires_at,
            next_run_at,
            schedule: TaskSchedule::Cron(schedule),
        };
        let info = ScheduledTaskInfo::from_task(&task);

        let mut tasks = self.tasks.lock().await;
        ensure_task_capacity(tasks.len())?;
        tasks.insert(task.id.clone(), task);
        Ok(info)
    }

    pub(crate) async fn create_once(
        &self,
        run_at: DateTime<Utc>,
        prompt: &str,
        now: DateTime<Utc>,
    ) -> Result<ScheduledTaskInfo, String> {
        let prompt = normalize_prompt(prompt)?;
        let id = self.next_task_id();
        let task = ScheduledTask {
            next_run_at: apply_one_shot_jitter(&id, run_at),
            id,
            prompt,
            created_at: now,
            expires_at: None,
            schedule: TaskSchedule::Once { run_at },
        };
        let info = ScheduledTaskInfo::from_task(&task);

        let mut tasks = self.tasks.lock().await;
        ensure_task_capacity(tasks.len())?;
        tasks.insert(task.id.clone(), task);
        Ok(info)
    }

    pub(crate) async fn list(&self) -> Vec<ScheduledTaskInfo> {
        let mut tasks = self
            .tasks
            .lock()
            .await
            .values()
            .map(ScheduledTaskInfo::from_task)
            .collect::<Vec<_>>();
        tasks.sort_by_key(|task| (task.next_run_at, task.id.clone()));
        tasks
    }

    pub(crate) async fn delete(&self, id: &str) -> Option<ScheduledTaskInfo> {
        self.tasks
            .lock()
            .await
            .remove(id)
            .map(|task| ScheduledTaskInfo::from_task(&task))
    }

    pub(crate) async fn take_due_task(&self, now: DateTime<Utc>) -> Option<DueTask> {
        let mut tasks = self.tasks.lock().await;

        loop {
            let due_id = tasks
                .values()
                .filter(|task| task.next_run_at <= now)
                .min_by_key(|task| task.next_run_at)
                .map(|task| task.id.clone());
            let due_id = due_id?;

            let Some(mut task) = tasks.remove(&due_id) else {
                continue;
            };
            let prompt = task.prompt.clone();

            match &task.schedule {
                TaskSchedule::Once { .. } => {}
                TaskSchedule::Cron(schedule) => {
                    if let Some(next_run_at) = schedule.next_run_at(&task.id, now, task.expires_at)
                    {
                        task.next_run_at = next_run_at;
                        tasks.insert(task.id.clone(), task);
                    }
                }
            }

            return Some(DueTask { id: due_id, prompt });
        }
    }

    fn next_task_id(&self) -> String {
        let next_id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        format!("{next_id:08x}")
    }
}

fn normalize_prompt(prompt: &str) -> Result<String, String> {
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return Err("prompt must not be empty".to_string());
    }
    Ok(prompt.to_string())
}

fn ensure_task_capacity(task_count: usize) -> Result<(), String> {
    if task_count >= MAX_TASKS {
        return Err(format!("scheduled task limit reached ({MAX_TASKS})"));
    }
    Ok(())
}

fn stable_offset_seconds(id: &str, max_seconds: i64) -> i64 {
    if max_seconds <= 0 {
        return 0;
    }

    let mut hasher = DefaultHasher::new();
    id.hash(&mut hasher);
    i64::try_from(hasher.finish() % u64::try_from(max_seconds + 1).unwrap_or(1)).unwrap_or(0)
}

fn should_apply_one_shot_jitter(run_at: DateTime<Utc>) -> bool {
    run_at.second() == 0 && matches!(run_at.minute(), 0 | 30)
}

fn apply_one_shot_jitter(id: &str, run_at: DateTime<Utc>) -> DateTime<Utc> {
    if !should_apply_one_shot_jitter(run_at) {
        return run_at;
    }

    let early_seconds = stable_offset_seconds(id, MAX_ONE_SHOT_EARLY_JITTER_SECONDS);
    run_at - ChronoDuration::seconds(early_seconds)
}

#[derive(Clone, Debug)]
struct CronSchedule {
    original: String,
    minutes: CronField,
    hours: CronField,
    days_of_month: CronField,
    months: CronField,
    days_of_week: CronField,
    day_of_month_is_wildcard: bool,
    day_of_week_is_wildcard: bool,
}

impl CronSchedule {
    fn parse(expression: &str) -> Result<Self, String> {
        let parts = expression.split_whitespace().collect::<Vec<_>>();
        if parts.len() != 5 {
            return Err(
                "schedule must be a 5-field cron expression: minute hour day-of-month month day-of-week"
                    .to_string(),
            );
        }

        let minutes = CronField::parse(
            parts[0], /*min*/ 0, /*max*/ 59, /*normalize_sunday*/ false,
        )?;
        let hours = CronField::parse(
            parts[1], /*min*/ 0, /*max*/ 23, /*normalize_sunday*/ false,
        )?;
        let days_of_month = CronField::parse(
            parts[2], /*min*/ 1, /*max*/ 31, /*normalize_sunday*/ false,
        )?;
        let months = CronField::parse(
            parts[3], /*min*/ 1, /*max*/ 12, /*normalize_sunday*/ false,
        )?;
        let days_of_week = CronField::parse(
            parts[4], /*min*/ 0, /*max*/ 7, /*normalize_sunday*/ true,
        )?;

        Ok(Self {
            original: expression.trim().to_string(),
            minutes,
            hours,
            days_of_month,
            months,
            days_of_week,
            day_of_month_is_wildcard: parts[2] == "*",
            day_of_week_is_wildcard: parts[4] == "*",
        })
    }

    fn next_run_at(
        &self,
        task_id: &str,
        after: DateTime<Utc>,
        expires_at: Option<DateTime<Utc>>,
    ) -> Option<DateTime<Utc>> {
        let nominal = self.next_nominal_run_at(after)?;
        if let Some(expires_at) = expires_at
            && nominal > expires_at
        {
            return None;
        }

        Some(nominal + ChronoDuration::seconds(self.recurring_jitter_seconds(task_id, nominal)))
    }

    fn recurring_jitter_seconds(&self, task_id: &str, nominal: DateTime<Utc>) -> i64 {
        let Some(next_nominal) = self.next_nominal_run_at(nominal) else {
            return 0;
        };
        let period_seconds = (next_nominal - nominal).num_seconds().max(0);
        let max_jitter_seconds = (period_seconds / 10).min(MAX_RECURRING_JITTER_SECONDS);
        stable_offset_seconds(task_id, max_jitter_seconds)
    }

    fn next_nominal_run_at(&self, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let base = after.with_timezone(&Local);
        let next_local = self.next_after_local(base)?;
        Some(next_local.with_timezone(&Utc))
    }

    fn next_after_local(&self, after: DateTime<Local>) -> Option<DateTime<Local>> {
        let mut candidate = align_to_next_minute(after);
        for _ in 0..MAX_SEARCH_MINUTES {
            if self.matches(candidate) {
                return Some(candidate);
            }
            candidate += ChronoDuration::minutes(1);
        }
        None
    }

    fn matches(&self, candidate: DateTime<Local>) -> bool {
        let minute = candidate.minute();
        let hour = candidate.hour();
        let day_of_month = candidate.day();
        let month = candidate.month();
        let day_of_week = candidate.weekday().num_days_from_sunday();

        if !self.minutes.contains(minute)
            || !self.hours.contains(hour)
            || !self.months.contains(month)
        {
            return false;
        }

        let day_of_month_matches = self.days_of_month.contains(day_of_month);
        let day_of_week_matches = self.days_of_week.contains(day_of_week);

        if self.day_of_month_is_wildcard && self.day_of_week_is_wildcard {
            return true;
        }
        if self.day_of_month_is_wildcard {
            return day_of_week_matches;
        }
        if self.day_of_week_is_wildcard {
            return day_of_month_matches;
        }

        day_of_month_matches || day_of_week_matches
    }
}

#[derive(Clone, Debug)]
struct CronField {
    min: u32,
    max: u32,
    allowed: Vec<bool>,
}

impl CronField {
    fn parse(expression: &str, min: u32, max: u32, normalize_sunday: bool) -> Result<Self, String> {
        let mut field = Self {
            min,
            max,
            allowed: vec![false; usize::try_from(max + 1).map_err(|_| "range is too large")?],
        };

        for token in expression.split(',') {
            field.add_token(token.trim(), normalize_sunday)?;
        }

        if field.allowed.iter().all(|allowed| !*allowed) {
            return Err(format!("cron field `{expression}` produced no values"));
        }

        Ok(field)
    }

    fn add_token(&mut self, token: &str, normalize_sunday: bool) -> Result<(), String> {
        if token.is_empty() {
            return Err("cron field contains an empty token".to_string());
        }

        let (range_expression, step) = if let Some((range_expression, step)) = token.split_once('/')
        {
            let step = step
                .parse::<u32>()
                .map_err(|_| format!("invalid cron step `{step}`"))?;
            if step == 0 {
                return Err("cron step must be greater than zero".to_string());
            }
            (range_expression, step)
        } else {
            (token, 1)
        };

        let (start, end) = if range_expression == "*" {
            (self.min, self.max)
        } else if let Some((start, end)) = range_expression.split_once('-') {
            (
                self.parse_value(start, normalize_sunday)?,
                self.parse_value(end, normalize_sunday)?,
            )
        } else {
            let start = self.parse_value(range_expression, normalize_sunday)?;
            if token.contains('/') {
                (start, self.max)
            } else {
                (start, start)
            }
        };

        if start > end {
            return Err(format!("invalid cron range `{token}`"));
        }

        let mut value = start;
        while value <= end {
            self.allowed
                [usize::try_from(value).map_err(|_| format!("invalid cron value `{value}`"))?] =
                true;
            match value.checked_add(step) {
                Some(next) => value = next,
                None => break,
            }
        }

        Ok(())
    }

    fn parse_value(&self, value: &str, normalize_sunday: bool) -> Result<u32, String> {
        let mut parsed = value
            .parse::<u32>()
            .map_err(|_| format!("invalid cron value `{value}`"))?;
        if normalize_sunday && parsed == 7 {
            parsed = 0;
        }
        if parsed < self.min || parsed > self.max {
            return Err(format!(
                "cron value `{parsed}` is outside the supported range {}-{}",
                self.min, self.max
            ));
        }
        Ok(parsed)
    }

    fn contains(&self, value: u32) -> bool {
        usize::try_from(value)
            .ok()
            .and_then(|index| self.allowed.get(index))
            .copied()
            .unwrap_or(false)
    }
}

fn align_to_next_minute(after: DateTime<Local>) -> DateTime<Local> {
    let next = after + ChronoDuration::minutes(1);
    match Local.with_ymd_and_hms(
        next.year(),
        next.month(),
        next.day(),
        next.hour(),
        next.minute(),
        0,
    ) {
        LocalResult::Single(value) => value,
        LocalResult::Ambiguous(earliest, _) => earliest,
        LocalResult::None => next,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use pretty_assertions::assert_eq;

    fn local_datetime(
        year: i32,
        month: u32,
        day: u32,
        hour: u32,
        minute: u32,
        second: u32,
    ) -> DateTime<Local> {
        match Local.with_ymd_and_hms(year, month, day, hour, minute, second) {
            LocalResult::Single(value) => value,
            LocalResult::Ambiguous(earliest, _) => earliest,
            LocalResult::None => panic!("invalid local datetime"),
        }
    }

    #[test]
    fn cron_schedule_finds_next_step_minute() {
        let schedule = CronSchedule::parse("*/15 * * * *").expect("parse cron");
        let start = local_datetime(2026, 1, 2, 10, 7, 30);
        let next = schedule.next_after_local(start).expect("next run");

        assert_eq!(next.minute(), 15);
        assert_eq!(next.hour(), 10);
    }

    #[test]
    fn cron_schedule_supports_weekday_ranges() {
        let schedule = CronSchedule::parse("0 9 * * 1-5").expect("parse cron");
        let start = local_datetime(2026, 1, 2, 10, 0, 0);
        let next = schedule.next_after_local(start).expect("next run");

        assert_eq!(next.hour(), 9);
        assert_eq!(next.minute(), 0);
        assert_eq!(next.weekday().num_days_from_sunday(), 1);
    }

    #[test]
    fn recurring_jitter_is_bounded_by_ten_percent() {
        let schedule = CronSchedule::parse("* * * * *").expect("parse cron");
        let now = Utc.with_ymd_and_hms(2026, 1, 2, 10, 7, 30).unwrap();
        let nominal = schedule.next_nominal_run_at(now).expect("nominal run");
        let jittered = schedule
            .next_run_at("deadbeef", now, Some(now + ChronoDuration::hours(72)))
            .expect("jittered run");

        assert_eq!(nominal.minute(), 8);
        assert!(jittered >= nominal);
        assert!(jittered <= nominal + ChronoDuration::seconds(6));
    }

    #[tokio::test]
    async fn one_shot_tasks_are_removed_after_firing() {
        let tasks = ScheduledTasks::default();
        let now = Utc::now();
        let run_at = now - ChronoDuration::seconds(1);
        tasks
            .create_once(run_at, "ping the user", now)
            .await
            .expect("create task");

        let due = tasks.take_due_task(Utc::now()).await.expect("due task");
        assert_eq!(due.prompt, "ping the user");
        assert!(tasks.list().await.is_empty());
    }

    #[tokio::test]
    async fn recurring_tasks_fire_once_after_overdue_gap() {
        let tasks = ScheduledTasks::default();
        let now = Utc::now();
        let created = tasks
            .create_cron("* * * * *", "check status", now)
            .await
            .expect("create task");
        {
            let mut guard = tasks.tasks.lock().await;
            let task = guard.get_mut(&created.id).expect("stored task");
            task.next_run_at = now - ChronoDuration::hours(6);
        }

        let due = tasks.take_due_task(now).await.expect("due task");
        assert_eq!(due.prompt, "check status");

        let remaining = tasks.list().await;
        assert_eq!(remaining.len(), 1);
        assert!(remaining[0].next_run_at > now);
    }

    #[tokio::test]
    async fn recurring_tasks_expire_after_final_run() {
        let tasks = ScheduledTasks::default();
        let now = Utc::now();
        let created = tasks
            .create_cron("0 * * * *", "final status check", now)
            .await
            .expect("create task");
        {
            let mut guard = tasks.tasks.lock().await;
            let task = guard.get_mut(&created.id).expect("stored task");
            task.next_run_at = now - ChronoDuration::minutes(1);
            task.expires_at = Some(now);
        }

        let due = tasks.take_due_task(now).await.expect("due task");
        assert_eq!(due.prompt, "final status check");
        assert!(tasks.list().await.is_empty());
    }

    #[tokio::test]
    async fn one_shot_half_hour_runs_can_fire_early() {
        let tasks = ScheduledTasks::default();
        let now = Utc::now();
        let run_at = Utc.with_ymd_and_hms(2026, 1, 2, 10, 30, 0).unwrap();
        let task = tasks
            .create_once(run_at, "half hour check", now)
            .await
            .expect("create task");

        assert!(task.next_run_at <= run_at);
        assert!(task.next_run_at >= run_at - ChronoDuration::seconds(90));
    }

    #[tokio::test]
    async fn scheduled_tasks_are_capped_at_fifty() {
        let tasks = ScheduledTasks::default();
        let now = Utc::now();
        for index in 0..MAX_TASKS {
            tasks
                .create_once(
                    now + ChronoDuration::minutes(i64::try_from(index).unwrap_or(0)),
                    "ping",
                    now,
                )
                .await
                .expect("create task within limit");
        }

        let err = tasks
            .create_once(now + ChronoDuration::minutes(60), "overflow", now)
            .await
            .expect_err("task limit error");
        assert_eq!(err, format!("scheduled task limit reached ({MAX_TASKS})"));
    }

    #[tokio::test]
    async fn scheduled_task_ids_are_eight_hex_characters() {
        let tasks = ScheduledTasks::default();
        let task = tasks
            .create_once(Utc::now(), "ping", Utc::now())
            .await
            .expect("create task");

        assert_eq!(task.id.len(), 8);
        assert!(
            task.id
                .chars()
                .all(|character| character.is_ascii_hexdigit())
        );
    }
}
