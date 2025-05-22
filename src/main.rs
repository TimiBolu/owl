// src/main.rs
mod ast;
mod executor;
mod parser;
mod tokenizer;

use executor::execute;
use parser::Parser;
use std::error::Error;
use tokenizer::tokenize;

fn main() -> Result<(), Box<dyn Error>> {
    let db = sled::open("owl_db_data")?;

    let program_str = r#"
        SatelliteImagery {
            coordinates GeoPoint(grid: { resolution: "1km" })
            fire_detected Bool
            temperature Float
            timestamp DateTime
        }

        Hospitals {
            location GeoPoint
            name String
            bed_capacity Int
            emergency_contact String
        }

        Banks {
            location GeoPoint
            name String
        }

        enum SchoolKind {
            Public,
            Private
        }

        Schools {
            location GeoPoint
            name String
            student_count Int
            kind SchoolKind
        }

        type GeoGrid {
            grid_id String
        }

        type Range {
            min Number
            max Number
        }

        Employee {
            id String
            name String
            age Int8
            profession String
            location GeoPoint(grid: { resolution: "1km" })
        }

        Profession {
            id String
            name String
            industry String
        }

        Industry {
            id String
            name String
        }

        fn count_one_km_in_zone(Coll Collection<{location GeoPoint}>, zone GeoGrid) -> Number {
            Coll::filter {
                geo.grid_cell(.location, "1km") == zone.grid_id
            }::count { .location }
        }

        fn locationInZone(Coll Collection<{location GeoPoint}>, zone GeoGrid) -> Collection<{location GeoPoint}> {
            Coll::filter {
                geo.grid_cell(.location, "1km") == zone.grid_id
            }
        }

        fn count_in_zone(Coll Collection<{location GeoPoint}>, zone GeoGrid) -> Number {
            locationInZone(Coll, zone)::count { .location }
        }

        fn students_in_zone(zone GeoGrid) -> Number {
            locationInZone(LargePublicSchools, zone)::sum { .student_count }
        }

        fn within_range(value Number, range Range) -> Bool {
            value >= range.min && value <= range.max
        }

        EmployeeData = Employee::batch_insert [
            { id: "e1", name: "Alice", age: 55, profession: "p1", location: { lat: 37.7749, lon: -122.4194 } },
            { id: "e2", name: "Bob", age: 45, profession: "p2", location: { lat: 40.7128, lon: -74.0060 } },
            { id: "e3", name: "Charlie", age: 60, profession: "p3", location: { lat: 51.5074, lon: -0.1278 } }
        ]

        ProfessionData = Profession::batch_insert [
            { id: "p1", name: "Engineer", industry: "i1" },
            { id: "p2", name: "Designer", industry: "i2" },
            { id: "p3", name: "Manager", industry: "i1" }
        ]

        IndustryData = Industry::batch_insert [
            { id: "i1", name: "Tech" },
            { id: "i2", name: "Creative" }
        ]

        SatelliteData = SatelliteImagery::batch_insert [
            { coordinates: { lat: 37.7749, lon: -122.4194 }, fire_detected: true, temperature: 35.5, timestamp: "2023-10-01T12:00:00Z" },
            { coordinates: { lat: 40.7128, lon: -74.0060 }, fire_detected: false, temperature: 22.3, timestamp: "2023-10-01T12:05:00Z" },
            { coordinates: { lat: 51.5074, lon: -0.1278 }, fire_detected: true, temperature: 40.0, timestamp: "2023-10-01T12:10:00Z" }
        ]

        HospitalData = Hospitals::batch_insert [
            { location: { lat: 37.7859, lon: -122.4364 }, name: "SF General", bed_capacity: 200, emergency_contact: "911" },
            { location: { lat: 40.7228, lon: -74.0160 }, name: "NY Medical", bed_capacity: 150, emergency_contact: "911" }
        ]

        BankData = Banks::batch_insert [
            { location: { lat: 37.7959, lon: -122.4264 }, name: "Bank of SF" },
            { location: { lat: 40.7328, lon: -74.0260 }, name: "NY Bank" }
        ]

        SchoolData = Schools::batch_insert [
            { location: { lat: 37.7649, lon: -122.4094 }, name: "SF Public", student_count: 1500, kind: "Public" },
            { location: { lat: 40.7028, lon: -74.0160 }, name: "NY Private", student_count: 800, kind: "Private" }
        ]

        LargePublicSchools = Schools::filter {
            .kind == "Public"
            .student_count > 1000
        }

        SeniorEmployees = EmployeeData::filter {
            .age > 50
        }::geo_within {
            field: .location,
            bounds: { min_lat: 30.0, max_lat: 60.0, min_lon: -130.0, max_lon: 0.0 }
        }::lead {
            name: next_employee,
            field: .name,
            offset: 1,
            order by .age asc
        }::select {
            name: .name,
            years_to_retirement: 65 - .age,
            industry: .profession.industry.name,
            next_employee,
            employee_count: count(.id)
        }

        EmergencyReport = SatelliteData::group {
            by geo.grid_cell(.coordinates, "1km") as zone
        }::select {
            hospitals: count_in_zone(HospitalData, zone),
            banks: count_in_zone(BankData, zone),
            school_count: count_in_zone(LargePublicSchools, zone),
            total_students: students_in_zone(zone),
            evacuation_capacity: (hospitals * 50 + banks * 10) / total_students
        }::filter {
            total_students > 0
        }

        sys::result {
            seniors: SeniorEmployees,
            emergency: EmergencyReport
        }
    "#.trim();

    println!("Parsing program: {}", program_str);
    let tokens = tokenize(program_str);
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program()?;
    println!("AST: {:#?}", program);
    let result = execute(program, &db)?;
    println!("Result: {:#?}", result);

    Ok(())
}
