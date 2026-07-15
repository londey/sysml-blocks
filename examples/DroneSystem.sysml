package DroneSystem {
    doc /* Multidisciplinary quadcopter example: mechanical, electrical,
           and software components with cross-discipline interfaces. */

    import Definitions::*;

    part def Quadcopter {
        doc /* Top-level system: the flying vehicle. */

        attribute mass : MassValue = 1.35;
        attribute maxFlightTime : TimeValue = 25.0;

        part frame : Airframe;
        part battery : BatteryPack;
        part fc : FlightController;
        part motors : BrushlessMotor[4];
        part props : Propeller[4];

        port gcsLink : TelemetryPort;

        connect battery.powerOut to fc.powerIn;
        connect fc.escBus to motors.drive;
    }

    part def Airframe {
        doc /* Mechanical discipline: carbon frame. */
        attribute armLength : LengthValue = 0.22;
        attribute material : String = "carbon fiber";
    }

    part def BatteryPack {
        attribute capacity : ChargeValue = 5200.0;
        attribute cells : Integer = 4;
        port powerOut : PowerPort;
    }

    part def FlightController {
        doc /* Electrical + software discipline boundary. */
        part imu : InertialSensor;
        part sw : FlightSoftware;
        port powerIn : ~PowerPort;
        port escBus : EscPort;
    }

    part def BrushlessMotor {
        attribute kv : Real = 2300.0;
        port drive : ~EscPort;
    }

    part def Propeller {
        attribute diameter : LengthValue = 0.127;
    }

    part def InertialSensor {
        attribute sampleRate : Real = 1000.0;
    }

    part def FlightSoftware {
        doc /* Software discipline: control loops and telemetry. */
        action def StabilizeLoop;
        action stabilize : StabilizeLoop;
        state def FlightMode;
    }
}
