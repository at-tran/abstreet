use std::collections::{BTreeMap, HashMap, VecDeque};

use serde::{Deserialize, Serialize};

use abstutil::Counter;
use geom::{Duration, Time};
use map_model::{
    BusRouteID, BusStopID, CompressedMovementID, IntersectionID, LaneID, Map, MovementID,
    ParkingLotID, Path, PathRequest, RoadID, Traversable, TurnID,
};

use crate::{
    AgentID, AgentType, AlertLocation, CarID, Event, ParkingSpot, TripID, TripMode, TripPhaseType,
};
use downcast_rs::__std::collections::hash_map::Entry;
use std::hash::Hash;

/// As a simulation runs, different pieces emit Events. The Analytics object listens to these,
/// organizing and storing some information from them. The UI queries Analytics to draw time-series
/// and display statistics.
///
/// For all maps whose weekday scenario fully runs, the game's release includes some "prebaked
/// results." These are just serialized Analytics after running the simulation on a map without any
/// edits for the full day. This is the basis of A/B testing -- the player can edit the map, start
/// running the simulation, and compare the live Analytics to the prebaked baseline Analytics.
#[derive(Clone, Serialize, Deserialize)]
pub struct Analytics {
    pub road_thruput: TimeSeriesCount<RoadID>,
    pub intersection_thruput: TimeSeriesCount<IntersectionID>,
    // TODO For traffic signals, intersection_thruput could theoretically use this. But that
    // requires occasionally expensive or complicated summing or merging over all directions of an
    // intersection. So for now, eat the file size cost.
    pub traffic_signal_thruput: TimeSeriesCount<CompressedMovementID>,

    /// Most fields in Analytics are cumulative over time, but this is just for the current moment
    /// in time.
    pub demand: BTreeMap<MovementID, usize>,

    // TODO Reconsider this one
    pub bus_arrivals: Vec<(Time, CarID, BusRouteID, BusStopID)>,
    /// For each passenger boarding, how long did they wait at the stop?
    pub passengers_boarding: BTreeMap<BusStopID, Vec<(Time, BusRouteID, Duration)>>,
    pub passengers_alighting: BTreeMap<BusStopID, Vec<(Time, BusRouteID)>>,

    pub started_trips: BTreeMap<TripID, Time>,
    /// Finish time, ID, mode, trip duration if successful (or None if cancelled)
    pub finished_trips: Vec<(Time, TripID, TripMode, Option<Duration>)>,

    /// Records how long was spent waiting at each turn (Intersection) for a given trip
    /// Over a certain threshold
    /// TripID, [(TurnID, Time Waiting In Seconds)]
    pub trip_intersection_delays: BTreeMap<TripID, BTreeMap<TurnID, u8>>,
    /// Records the average speed/maximum speed for each lane
    /// If it is over a certain threshold (<95% of max speed)
    /// TripID, [(LaneID, Percent of maximum speed as an integer (0-100)]
    pub lane_speed_percentage: BTreeMap<TripID, BTreeMap<LaneID, u8>>,

    // TODO This subsumes finished_trips
    pub trip_log: Vec<(Time, TripID, Option<PathRequest>, TripPhaseType)>,

    // TODO Transit riders aren't represented here yet, just the vehicle they're riding.
    /// Only for traffic signals. The u8 is the movement index from a CompressedMovementID.
    pub intersection_delays: BTreeMap<IntersectionID, Vec<(u8, Time, Duration, AgentType)>>,

    /// Per parking lane or lot, when does a spot become filled (true) or free (false)
    pub parking_lane_changes: BTreeMap<LaneID, Vec<(Time, bool)>>,
    pub parking_lot_changes: BTreeMap<ParkingLotID, Vec<(Time, bool)>>,

    pub(crate) alerts: Vec<(Time, AlertLocation, String)>,

    /// For benchmarking, we may want to disable collecting data.
    record_anything: bool,
}

impl Analytics {
    pub fn new(record_anything: bool) -> Analytics {
        Analytics {
            road_thruput: TimeSeriesCount::new(),
            intersection_thruput: TimeSeriesCount::new(),
            traffic_signal_thruput: TimeSeriesCount::new(),
            demand: BTreeMap::new(),
            bus_arrivals: Vec::new(),
            passengers_boarding: BTreeMap::new(),
            passengers_alighting: BTreeMap::new(),
            started_trips: BTreeMap::new(),
            finished_trips: Vec::new(),
            trip_intersection_delays: BTreeMap::new(),
            lane_speed_percentage: BTreeMap::new(),
            trip_log: Vec::new(),
            intersection_delays: BTreeMap::new(),
            parking_lane_changes: BTreeMap::new(),
            parking_lot_changes: BTreeMap::new(),
            alerts: Vec::new(),
            record_anything,
        }
    }

    pub fn event(&mut self, ev: Event, time: Time, map: &Map) {
        if !self.record_anything {
            return;
        }

        // Throughput
        if let Event::AgentEntersTraversable(a, to, passengers) = ev {
            match to {
                Traversable::Lane(l) => {
                    self.road_thruput
                        .record(time, map.get_l(l).parent, a.to_type(), 1);
                    if let Some(n) = passengers {
                        self.road_thruput.record(
                            time,
                            map.get_l(l).parent,
                            AgentType::TransitRider,
                            n,
                        );
                    }
                }
                Traversable::Turn(t) => {
                    self.intersection_thruput
                        .record(time, t.parent, a.to_type(), 1);
                    if let Some(n) = passengers {
                        self.intersection_thruput.record(
                            time,
                            t.parent,
                            AgentType::TransitRider,
                            n,
                        );
                    }

                    if let Some(id) = map.get_movement(t) {
                        *self.demand.entry(id).or_insert(0) -= 1;

                        let m = map.get_traffic_signal(t.parent).compressed_id(t);
                        self.traffic_signal_thruput.record(time, m, a.to_type(), 1);
                        if let Some(n) = passengers {
                            self.traffic_signal_thruput
                                .record(time, m, AgentType::TransitRider, n);
                        }
                    }
                }
            };
        }
        match ev {
            Event::PersonLeavesMap(_, maybe_a, i) => {
                // Ignore cancelled trips
                if let Some(a) = maybe_a {
                    self.intersection_thruput.record(time, i, a.to_type(), 1);
                }
            }
            Event::PersonEntersMap(_, a, i) => {
                self.intersection_thruput.record(time, i, a.to_type(), 1);
            }
            _ => {}
        }

        // Bus arrivals
        if let Event::BusArrivedAtStop(bus, route, stop) = ev {
            self.bus_arrivals.push((time, bus, route, stop));
        }

        // Passengers boarding/alighting
        if let Event::PassengerBoardsTransit(_, _, route, stop, waiting) = ev {
            self.passengers_boarding
                .entry(stop)
                .or_insert_with(Vec::new)
                .push((time, route, waiting));
        }
        if let Event::PassengerAlightsTransit(_, _, route, stop) = ev {
            self.passengers_alighting
                .entry(stop)
                .or_insert_with(Vec::new)
                .push((time, route));
        }

        // Started trips
        if let Event::TripPhaseStarting(id, _, _, _) = ev {
            self.started_trips.entry(id).or_insert(time);
        }

        // Finished trips
        if let Event::TripFinished {
            trip,
            mode,
            total_time,
            ..
        } = ev
        {
            self.finished_trips
                .push((time, trip, mode, Some(total_time)));
        } else if let Event::TripCancelled(id, mode) = ev {
            self.started_trips.entry(id).or_insert(time);
            self.finished_trips.push((time, id, mode, None));
        }

        // Trip Intersection delay
        if let Event::TripIntersectionDelay(trip_id, turn_id, agent, delay) = ev {
            match agent {
                AgentID::Car(_) => {
                    if delay > Duration::seconds(30.0) {
                        self.trip_intersection_delays
                            .entry(trip_id)
                            .or_insert_with(BTreeMap::new)
                            .insert(turn_id, delay.inner_seconds() as u8);
                    }
                }
                AgentID::Pedestrian(_) => {
                    if delay > Duration::seconds(15.0) {
                        self.trip_intersection_delays
                            .entry(trip_id)
                            .or_insert_with(BTreeMap::new)
                            .insert(turn_id, delay.inner_seconds() as u8);
                    }
                }
                AgentID::BusPassenger(_, _) => {}
            }
        }
        // Lane Speed
        if let Event::LaneSpeedPercentage(trip_id, lane_id, avg_speed, max_speed) = ev {
            let speed_percent: u8 = ((avg_speed / max_speed) * 100.0) as u8;
            if speed_percent < 95 {
                self.lane_speed_percentage
                    .entry(trip_id)
                    .or_insert_with(BTreeMap::new)
                    .insert(lane_id, speed_percent);
            }
        }

        // Intersection delays
        if let Event::IntersectionDelayMeasured(id, delay, agent) = ev {
            self.intersection_delays
                .entry(id.i)
                .or_insert_with(Vec::new)
                .push((id.idx, time, delay, agent.to_type()));
        }

        // Parking spot changes
        if let Event::CarReachedParkingSpot(_, spot) = ev {
            if let ParkingSpot::Onstreet(l, _) = spot {
                self.parking_lane_changes
                    .entry(l)
                    .or_insert_with(Vec::new)
                    .push((time, true));
            } else if let ParkingSpot::Lot(pl, _) = spot {
                self.parking_lot_changes
                    .entry(pl)
                    .or_insert_with(Vec::new)
                    .push((time, true));
            }
        }
        if let Event::CarLeftParkingSpot(_, spot) = ev {
            if let ParkingSpot::Onstreet(l, _) = spot {
                self.parking_lane_changes
                    .entry(l)
                    .or_insert_with(Vec::new)
                    .push((time, false));
            } else if let ParkingSpot::Lot(pl, _) = spot {
                self.parking_lot_changes
                    .entry(pl)
                    .or_insert_with(Vec::new)
                    .push((time, false));
            }
        }

        // TODO Kinda hacky, but these all consume the event, so kinda bundle em.
        match ev {
            Event::TripPhaseStarting(id, _, maybe_req, phase_type) => {
                self.trip_log.push((time, id, maybe_req, phase_type));
            }
            Event::TripCancelled(id, _) => {
                self.trip_log
                    .push((time, id, None, TripPhaseType::Cancelled));
            }
            Event::TripFinished { trip, .. } => {
                self.trip_log
                    .push((time, trip, None, TripPhaseType::Finished));
            }
            Event::PathAmended(path) => {
                self.record_demand(&path, map);
            }
            Event::Alert(loc, msg) => {
                self.alerts.push((time, loc, msg));
            }
            _ => {}
        }
    }

    pub fn record_demand(&mut self, path: &Path, map: &Map) {
        for step in path.get_steps() {
            if let Traversable::Turn(t) = step.as_traversable() {
                if let Some(id) = map.get_movement(t) {
                    *self.demand.entry(id).or_insert(0) += 1;
                }
            }
        }
    }

    // TODO If these ever need to be speeded up, just cache the histogram and index in the events
    // list.

    /// Ignores the current time. Returns None for cancelled trips.
    pub fn finished_trip_time(&self, trip: TripID) -> Option<Duration> {
        // TODO This is so inefficient!
        for (_, id, _, maybe_dt) in &self.finished_trips {
            if *id == trip {
                return maybe_dt.clone();
            }
        }
        None
    }

    /// Returns pairs of trip times for finished trips in both worlds. (ID, before, after, mode)
    pub fn both_finished_trips(
        &self,
        now: Time,
        before: &Analytics,
    ) -> Vec<(TripID, Duration, Duration, TripMode)> {
        let mut a = BTreeMap::new();
        for (t, id, _, maybe_dt) in &self.finished_trips {
            if *t > now {
                break;
            }
            if let Some(dt) = maybe_dt {
                a.insert(*id, *dt);
            }
        }

        let mut results = Vec::new();
        for (t, id, mode, maybe_dt) in &before.finished_trips {
            if *t > now {
                break;
            }
            if let Some(dt) = maybe_dt {
                if let Some(dt1) = a.remove(id) {
                    results.push((*id, *dt, dt1, *mode));
                }
            }
        }
        results
    }

    /// If calling on prebaked Analytics, be careful to pass in an unedited map, to match how the
    /// simulation was originally run. Otherwise the paths may be nonsense.
    pub fn get_trip_phases(&self, trip: TripID, map: &Map) -> Vec<TripPhase> {
        let mut phases: Vec<TripPhase> = Vec::new();
        for (t, id, maybe_req, phase_type) in &self.trip_log {
            if *id != trip {
                continue;
            }
            if let Some(ref mut last) = phases.last_mut() {
                last.end_time = Some(*t);
            }
            if *phase_type == TripPhaseType::Finished || *phase_type == TripPhaseType::Cancelled {
                break;
            }
            phases.push(TripPhase {
                start_time: *t,
                end_time: None,
                path: maybe_req.clone().and_then(|req| map.pathfind(req).ok()),
                has_path_req: maybe_req.is_some(),
                phase_type: *phase_type,
            })
        }
        phases
    }

    pub fn get_all_trip_phases(&self) -> BTreeMap<TripID, Vec<TripPhase>> {
        let mut trips = BTreeMap::new();
        for (t, id, maybe_req, phase_type) in &self.trip_log {
            let phases: &mut Vec<TripPhase> = trips.entry(*id).or_insert_with(Vec::new);
            if let Some(ref mut last) = phases.last_mut() {
                last.end_time = Some(*t);
            }
            if *phase_type == TripPhaseType::Finished {
                continue;
            }
            // Remove cancelled trips
            if *phase_type == TripPhaseType::Cancelled {
                trips.remove(id);
                continue;
            }
            phases.push(TripPhase {
                start_time: *t,
                end_time: None,
                // Don't compute any paths
                path: None,
                has_path_req: maybe_req.is_some(),
                phase_type: *phase_type,
            })
        }
        trips
    }

    pub fn active_agents(&self, now: Time) -> Vec<(Time, usize)> {
        let mut starts_stops: Vec<(Time, bool)> = Vec::new();
        for t in self.started_trips.values() {
            if *t <= now {
                starts_stops.push((*t, false));
            }
        }
        for (t, _, _, _) in &self.finished_trips {
            if *t > now {
                break;
            }
            starts_stops.push((*t, true));
        }
        // Make sure the start events get sorted before the stops.
        starts_stops.sort();

        let mut pts = Vec::new();
        let mut cnt = 0;
        let mut last_t = Time::START_OF_DAY;
        for (t, ended) in starts_stops {
            if t != last_t {
                // Step functions. Don't interpolate.
                pts.push((last_t, cnt));
            }
            last_t = t;
            if ended {
                // release mode disables this check, so...
                if cnt == 0 {
                    panic!("active_agents at {} has more ended trips than started", t);
                }
                cnt -= 1;
            } else {
                cnt += 1;
            }
        }
        pts.push((last_t, cnt));
        if last_t != now {
            pts.push((now, cnt));
        }
        pts
    }

    /// Returns the free spots over time
    pub fn parking_lane_availability(
        &self,
        now: Time,
        l: LaneID,
        capacity: usize,
    ) -> Vec<(Time, usize)> {
        if let Some(changes) = self.parking_lane_changes.get(&l) {
            Analytics::parking_spot_availability(now, changes, capacity)
        } else {
            vec![(Time::START_OF_DAY, capacity), (now, capacity)]
        }
    }
    pub fn parking_lot_availability(
        &self,
        now: Time,
        pl: ParkingLotID,
        capacity: usize,
    ) -> Vec<(Time, usize)> {
        if let Some(changes) = self.parking_lot_changes.get(&pl) {
            Analytics::parking_spot_availability(now, changes, capacity)
        } else {
            vec![(Time::START_OF_DAY, capacity), (now, capacity)]
        }
    }

    fn parking_spot_availability(
        now: Time,
        changes: &Vec<(Time, bool)>,
        capacity: usize,
    ) -> Vec<(Time, usize)> {
        let mut pts = Vec::new();
        let mut cnt = capacity;
        let mut last_t = Time::START_OF_DAY;

        for (t, filled) in changes {
            if *t > now {
                break;
            }
            if *t != last_t {
                // Step functions. Don't interpolate.
                pts.push((last_t, cnt));
            }
            last_t = *t;
            if *filled {
                if cnt == 0 {
                    panic!("parking_spot_availability at {} went below 0", t);
                }
                cnt -= 1;
            } else {
                cnt += 1;
            }
        }
        pts.push((last_t, cnt));
        if last_t != now {
            pts.push((now, cnt));
        }
        pts
    }
}

impl Default for Analytics {
    fn default() -> Analytics {
        Analytics::new(false)
    }
}

#[derive(Debug)]
pub struct TripPhase {
    pub start_time: Time,
    pub end_time: Option<Time>,
    pub path: Option<Path>,
    pub has_path_req: bool,
    pub phase_type: TripPhaseType,
}

/// See https://github.com/dabreegster/abstreet/issues/85
#[derive(Clone, Serialize, Deserialize)]
pub struct TimeSeriesCount<X: Ord + Clone + Hash + Eq> {
    /// (Road or intersection, type) -> Vec of sum of counts
    pub sum_counts: HashMap<(X, AgentType), Vec<u16>>,

    // The interval index of the previous time `record` was called
    // for each `id` and `agent_type`
    prev_indices: HashMap<(X, AgentType), usize>,

    thruput_interval: Duration,
    window_size: Duration,
}

impl<X: Ord + Clone + Hash + Eq> TimeSeriesCount<X> {
    fn new() -> TimeSeriesCount<X> {
        TimeSeriesCount {
            sum_counts: HashMap::new(),
            prev_indices: HashMap::new(),
            thruput_interval: Duration::minutes(1),
            window_size: Duration::minutes(30),
        }
    }

    /// Returns the interval index corresponding to `time`.
    /// Rounds down, so 00:00:00 and 00:00:59 are both in interval 0
    /// if the `self.thruput_interval` is 1 minute.
    fn time_to_interval(&self, time: Time) -> usize {
        ((time - Time::START_OF_DAY) / self.thruput_interval) as usize
    }

    /// This function assumes the provided `time` is later than all previous `time`
    /// passed into this function for each `id` and `agent_type` pair
    fn record(&mut self, time: Time, id: X, agent_type: AgentType, count: usize) {
        let interval = self.time_to_interval(time);
        let prev_index = *self
            .prev_indices
            .get(&(id.clone(), agent_type))
            .unwrap_or(&0);

        if interval < prev_index {
            panic!(
                "Can't record throughput count at interval {} \
                because it is earlier than interval {}",
                interval, prev_index
            );
        }

        match self.sum_counts.entry((id.clone(), agent_type)) {
            Entry::Occupied(ref mut entry) => {
                let sum_count = entry.get_mut();
                for i in (prev_index + 1)..interval {
                    assert_eq!(sum_count[i], 0);
                    sum_count[i] = sum_count[i - 1];
                }
                sum_count[interval] = sum_count[interval - 1] + count as u16;
            }
            Entry::Vacant(entry) => {
                let mut new_sum_counts =
                    vec![0; (Duration::hours(24) / self.thruput_interval) as usize];
                new_sum_counts[interval] = count as u16;
                entry.insert(new_sum_counts);
            }
        }

        *self.prev_indices.get_mut(&(id, agent_type)).unwrap() = interval;
    }

    /// Given a sum_count vector, returns the total number of counts for the day.
    /// This is equal ot the last non-zero value in the vector.
    fn total_from_sum_count(sum_count: &Vec<u16>) -> usize {
        for count in sum_count.iter().rev() {
            if *count != 0 {
                return *count as usize;
            }
        }
        return 0;
    }

    pub fn total_for(&self, id: X) -> usize {
        let mut cnt = 0;
        for agent_type in AgentType::all() {
            if let Some(sum_count) = self.sum_counts.get(&(id.clone(), agent_type)) {
                cnt += Self::total_from_sum_count(sum_count);
            }
        }
        cnt
    }

    pub fn all_total_counts(&self) -> Counter<X> {
        let mut cnt = Counter::new();
        for ((id, _), value) in &self.sum_counts {
            cnt.add(id.clone(), Self::total_from_sum_count(value));
        }
        cnt
    }

    pub fn throughput_vec(&self, now: Time, id: X) -> Vec<(AgentType, Vec<(Time, usize)>)> {
        let mut pts_per_type: BTreeMap<AgentType, Vec<(Time, usize)>> = BTreeMap::new();

        for agent_type in AgentType::all() {
            let mut pts = Vec::new();

            let mut t = Time::START_OF_DAY;
            while t < now {
                let interval_start = if t < Time::START_OF_DAY + self.window_size {
                    0
                } else {
                    self.time_to_interval(t - self.window_size)
                };
                let interval_end = self.time_to_interval(t);

                let count = self.sum_counts[&(id.clone(), agent_type)][interval_end]
                    - self.sum_counts[&(id.clone(), agent_type)][interval_start];
                pts.push((t, count as usize));
                t += self.thruput_interval;
            }

            pts_per_type.insert(agent_type, pts);
        }

        pts_per_type.into_iter().collect()
    }
}

pub struct Window {
    times: VecDeque<Time>,
    window_size: Duration,
}

impl Window {
    pub fn new(window_size: Duration) -> Window {
        Window {
            times: VecDeque::new(),
            window_size,
        }
    }

    /// Returns the count at time
    pub fn add(&mut self, time: Time) -> usize {
        self.times.push_back(time);
        self.count(time)
    }

    /// Grab the count at this time, but don't add a new time
    pub fn count(&mut self, end: Time) -> usize {
        while !self.times.is_empty() && end - *self.times.front().unwrap() > self.window_size {
            self.times.pop_front();
        }
        self.times.len()
    }
}
