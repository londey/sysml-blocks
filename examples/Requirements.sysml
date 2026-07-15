package DroneRequirements {
    doc /* System requirements spanning disciplines. */

    requirement def <REQ1> MassLimit {
        doc /* Total takeoff mass shall not exceed 1.5 kg. */
        attribute maxMass : Real = 1.5;
    }

    requirement def <REQ2> FlightEndurance {
        doc /* The vehicle shall hover for at least 20 minutes. */
        attribute minMinutes : Real = 20.0;
    }

    requirement massLimit : MassLimit;
    requirement endurance : FlightEndurance;
}
