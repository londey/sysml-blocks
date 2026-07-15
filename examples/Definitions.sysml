package Definitions {
    doc /* Shared value types and interface definitions. */

    attribute def MassValue;
    attribute def TimeValue;
    attribute def LengthValue;
    attribute def ChargeValue;

    port def PowerPort {
        attribute voltage : Real;
        attribute maxCurrent : Real;
    }

    port def EscPort;
    port def TelemetryPort;

    interface def PowerDistribution {
        end supplier : PowerPort;
        end consumer : ~PowerPort;
    }
}
