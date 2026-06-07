# CAN Devices on Other Platforms

{% hint style="info" %}
For firmware updates, we recommend getting an FCS One DC24V. The FCS One can be used to update all devices one by one to keep them future-proof.
{% endhint %}

## Overview

* Baud rate: 1 Mbps

{% hint style="danger" %}
This guide and the use of our sensors on other platforms is currently a beta project. Use at your own risk and feedback any issues or bugs to <support@brewtools.no>.
{% endhint %}

### Wiring & Connections

We use a 5-pin connector on all our CAN devices. For most devices, the 4th pin is not in use.

The devices require a stable 24V DC power supply.&#x20;

Wiring of each CAN line should be done with a single long line and short stubs to each sensor. Use a 120 $$\Omega$$ resistor shortly after the last device.

```
PIN 1 (red):    +24V
PIN 2 (yellow): CAN H
PIN 3 (white):  CAN L
PIN 4 (green):  Not Connected
PIN 5 (black):  Ground
```

### Setup Process

Our design support 8 of the same device type per CAN bus (node ID 0-7). All devices are shipped with node ID 0, so in order to have multiple devices on a single CAN bus you need to assign each one a unique node ID (described in detail in [#common-features](#common-features "mention")). The easiest way to do this, is to connect the first device, give it node ID 1, then connect the next one - give it node ID 2 and so on until you have reached 7, and then the last one can keep node ID 0. If you have 8 density sensors, the result should look like this:

1. Density sensor @ node id 0
2. Density sensor @ node id 1
3. Density sensor @ node id 2
4. Density sensor @ node id 3
5. Density sensor @ node id 4
6. Density sensor @ node id 5
7. Density sensor @ node id 6
8. Density sensor @ node id 7

The same procedure goes for other devices.

### CAN ID and Message Design

The CAN messages use the extended CAN ID format of 29 bits. It contains information about the message priority, sender node type, receiver node type, secondary node id and message type:

* `priority` (Priority: 2 bits - 4 values) = 2/29 bits
* `senderNodeType` (Sender Node Type: 8 bits - 256 values) = 10/29 bits
* `receiverNodeType` (Receiver Node Type: 8 bits - 256 values) = 18/29 bits
* `secondaryNodeId` (Secondary Node Identifier: 3 bits - 8 values) = 21/29 bits
* `msgType` (Message Type: 8 bits - 256 values) = 29/29 bits

To extract these fields from your received CAN message, use the following bit shifting scheme:

```cpp
/* Extract fields from CAN ID */
priority = (message.identifier >> 27) & 0x03;
senderNodeType = (message.identifier >> 19) & 0xFF;
receiverNodeType = (message.identifier >> 11) & 0xFF;
secondaryNodeId = (message.identifier >> 8) & 0x07;
msgType = message.identifier & 0xFF;
```

To create a new CAN ID, the following function can be used:

```cpp
static inline uint32_t getCANid(uint8_t priority, uint8_t senderNodeType,
 uint8_t receiverNodeType, uint8_t secondaryNodeId, uint8_t msgType) {
    uint32_t canId = (static_cast<uint32_t>(priority)         << 27) |
                     (static_cast<uint32_t>(senderNodeType)   << 19) |
                     (static_cast<uint32_t>(receiverNodeType) << 11) |
                     (static_cast<uint32_t>(secondaryNodeId)  << 8)  |
                     (static_cast<uint32_t>(msgType));
    return canId;
}
```

The first byte in the the CAN message is a sub-index, which is used to distinguish between values of the same type, e.g. temperature reading 1, 2 and 3, all using the same message type. The way to extract the data correctly is shown below.

```cpp
/* Extract sub-index (first byte of data) */
subIndex = message.data[0];

/* Copy raw data payload (excluding sub-index) to the provided buffer */
dataLength = message.length - 1;  // Data length without sub-index
memcpy(buffer, &message.data[1], dataLength);
```

So let's say we want to process an incoming CAN message containing a density value from a density sensor, we could do something like:

```cpp
#define MAX_NODES 8

float densitySg[MAX_NODES] = {0}; // Storage for each node's density value

if (can.receive(priority, senderNodeType, receiverNodeType, secondaryNodeId, msgType, subIndex, data, dataLength))
{
    switch (senderNodeType)
    {
        case NODE_TYPE_DENSITY_SENSOR:
        {
            switch (msgType)
            {
                case MSG_TYPE_DENSITY:
                {
                    /* Extract sub-index (first byte of data) */
                    subIndex = message.data[0]; // Line not necessary, just here for clarity

                    /* Copy the next "size of a float" bytes to your variable */
                    if (secondaryNodeId < MAX_NODES)
                    {
                        /* Copy incoming float into the correct array index for this node */
                        memcpy(&densitySg[secondaryNodeId], &message.data[1], sizeof(float));
                    }
                }
            }
        }
    }
}

```

### Common Features

These features are common for all CAN devices, and can be implemented as general functions in your application.

#### Update Sensor Node ID

The firmware on our devices support up to 8 of the same node type on the same bus.

Pseudo-code for implementing a function that sends a new node ID to a density sensor is shown below:

```cpp
bool updateDensitySensorId(uint32_t currentNodeId /* [0-7] */, uint32_t newNodeId /* [0-7] */)
{
    int subIndex = 0; /* Unused for this purpose, set to 0 */

    can_message_t message;
    message.identifier = getCANid(PRIORITY_MEDIUM, 
                                  NODE_TYPE_PLC, 
                                  NODE_TYPE_DENSITY_SENSOR, 
                                  currentNodeId, 
                                  MSG_TYPE_NODE_ID);
    message.extd = 1;
    message.length = dataLength;

    uint8_t data[5];
    data[0] = subIndex;
    data[1] = (newNodeId >> 24) & 0xFF;
    data[2] = (newNodeId >> 16) & 0xFF;
    data[3] = (newNodeId >> 8)  & 0xFF;
    data[4] =  newNodeId        & 0xFF;

    memcpy(message.data, data, dataLength);

    return(can.transmit(message));
}
```

Out of the box, your density sensor will have node ID 0. So to update it to 1, you would do `updateDensitySensorId(0, 1);` . To set it back to factory default (and make it compatible with FCS again) you would do `updateDensitySensorId(1, 0);` . Similarly, you can do this for other devices by changing the `NODE_TYPE_DENSITY_SENSOR`  to your desired node type. The sender node type in this case is `NODE_TYPE_PLC`  = 8.

#### Reading The Device Version

To read the version number (major.minor.patch, e.g. 1.2.1) of your CAN device, you can listen for message type `MSG_TYPE_SEMANTIC_VERSION` = 1, and decode the data as follows (skipping sub-index 0, so data is in bytes 1-6):

```cpp
uint16_t major = (uint16_t)data[1] | ((uint16_t)data[2] << 8);
uint16_t minor = (uint16_t)data[3] | ((uint16_t)data[4] << 8);
uint16_t patch = (uint16_t)data[5] | ((uint16_t)data[6] << 8);
```

#### Sending Data to a Device

A general function for sending bytes of data to your devices are shown below.

```cpp
bool sendCanData(uint8_t priority, uint8_t senderNodeType,
                     uint8_t receiverNodeType, uint8_t secondaryNodeId,
                     uint8_t msgType, uint8_t *data, size_t dataLength) {

    const uint32_t canId = (uint32_t(priority) << 27)
                         | (uint32_t(senderNodeType) << 19)
                         | (uint32_t(receiverNodeType) << 11)
                         | (uint32_t(secondaryNodeId) << 8)
                         | uint32_t(msgType);

    can_message_t message{};
    message.identifier = canId;
    message.length = (dataLength > 8) ? 8 : dataLength;
    std::memcpy(message.data, data, message.length);

    return(can.transmit(message));
}
```

Using this function, you can create a specific function for sending specific data types, as shown below:

```cpp
bool sendFloatCan(uint8_t priority, uint8_t senderNodeType, uint8_t receiverNodeType,
                            uint8_t secondaryNodeId, uint8_t msgType, float value) {
    uint8_t data[5];  // Adjusted to 5 bytes to include sub-index
    data[0] = 0;  // Set the sub-index as the first byte of the data array
    memcpy(&data[1], &value, sizeof(float)); // Copy float to the next 4 bytes
    return sendCanData(priority, senderNodeType, receiverNodeType, secondaryNodeId,
                       msgType, data, sizeof(data));
}
```

```cpp
bool sendUintCan(uint8_t priority, uint8_t senderNodeType, uint8_t receiverNodeType,
                           uint8_t secondaryNodeId, uint8_t msgType, uint32_t value) {
    uint8_t payload[5];
    payload[0] = 0; // subIndex
    payload[1] = uint8_t((value >> 24) & 0xFF);
    payload[2] = uint8_t((value >> 16) & 0xFF);
    payload[3] = uint8_t((value >> 8)  & 0xFF);
    payload[4] = uint8_t(value & 0xFF);
    return sendCanData(priority, senderNodeType, receiverNodeType, secondaryNodeId,
                       msgType, payload, sizeof(payload));
}
```

***

## Device Specific Configuration and Data

### Density / Temperature Sensor

The density/temperature sensor sends density on request (not at 10 ms interval) and constantly transmits temperature data. To start a new measurement, send a CAN message with the message type `MSG_TYPE_START_MEASUREMENT` , the data in the message can be left empty.

Calibration

To calibrate your density sensor, send the correct value from your EasyDense (or similar) as a float with the message type `MSG_TYPE_CALIBRATION_CMD`  using the SG unit (important!). You can then listen for the `MSG_TYPE_CALIBRATION_ACK`  message with acknowledgement types described below.

Node type: `NODE_TYPE_DENSITY_SENSOR = 4`

Message types

* To device
  * `MSG_TYPE_NODE_ID = 36`
  * `MSG_TYPE_START_MEASUREMENT_CMD = 33`
  * `MSG_TYPE_CALIBRATION_CMD = 28`
* From device
  * `MSG_TYPE_DENSITY = 14`  $$\[SG]$$
  * `MSG_TYPE_TEMPERATURE = 12`  $$\[°C]$$
  * `MSG_TYPE_CALIBRATION_ACK = 29`

Acknowledgement types

* `ACK_TYPE_NONE = 0`
* `ACK_TYPE_CALIBRATING = 1`
* `ACK_TYPE_OK = 2`
* `ACK_TYPE_ERROR = 3`

Examples

1. Receive density value (SG)

```cpp
#define MAX_NODES 8

float densitySg[MAX_NODES] = {0}; // Storage for each node's density value

if (can.receive(priority, senderNodeType, receiverNodeType, secondaryNodeId, msgType, subIndex, data, dataLength))
{
    switch (senderNodeType)
    {
        case NODE_TYPE_DENSITY_SENSOR:
        {
            switch (msgType)
            {
                case MSG_TYPE_DENSITY:
                {
                    /* Extract sub-index (first byte of data) */
                    subIndex = message.data[0]; // Line not necessary, just here for clarity

                    /* Copy the next "size of a float" bytes to your variable */
                    if (secondaryNodeId < MAX_NODES)
                    {
                        /* Copy incoming float into the correct array index for this node */
                        memcpy(&densitySg[secondaryNodeId], &message.data[1], sizeof(float));
                    }
                }
            }
        }
    }
}
```

2. Receive density sensor temperature value $$\[°C]$$

```cpp
#define MAX_NODES 8

float densityTemperature[MAX_NODES] = {0}; // Storage for each node's density value

if (can.receive(priority, senderNodeType, receiverNodeType, secondaryNodeId, msgType, subIndex, data, dataLength))
{
    switch (senderNodeType)
    {
        case NODE_TYPE_DENSITY_SENSOR:
        {
            switch (msgType)
            {
                case MSG_TYPE_TEMPERATURE:
                {
                    /* Extract sub-index (first byte of data) */
                    subIndex = message.data[0]; // Line not necessary, just here for clarity

                    /* Copy the next "size of a float" bytes to your variable */
                    if (secondaryNodeId < MAX_NODES)
                    {
                        /* Copy incoming float into the correct array index for this node */
                        memcpy(&densityTemperature[secondaryNodeId], &message.data[1], sizeof(float));
                    }
                }
            }
        }
    }
}
```

3. Send new calibration to sensor @ node ID 3

```cpp
float originalGravity = 1.054;
int nodeId = 3;
can.sendFloatCan(PRIORITY_HIGH, NODE_TYPE_PLC, NODE_TYPE_DENSITY_SENSOR,
                 nodeId, MSG_TYPE_CALIBRATION_CMD, originalGravity);
```

4. Receive calibration ack (pseudo-code, extend as you like)

```cpp
bool densitySensorCalibrated = false;

if (can.receive(priority, senderNodeType, receiverNodeType, secondaryNodeId, msgType, subIndex, data, dataLength))
{
    switch (senderNodeType)
    {
        case NODE_TYPE_DENSITY_SENSOR:
        {
            switch (msgType)
            {
                case MSG_TYPE_CALIBRATION_ACK:
                {
                    /* Extract sub-index (first byte of data) */
                    subIndex = message.data[0]; // Line not necessary, just here for clarity

                    if (secondaryNodeId < MAX_NODES)
                    {
                        const uint32_t ack = 
                          (uint32_t(data[1]) << 24)
                        | (uint32_t(data[2]) << 16)
                        | (uint32_t(data[3]) << 8)
                        |  uint32_t(data[4]);
                                          
                        if(ack == ACK_TYPE_OK)
                        {
                            densitySensorCalibrated = true;
                        }
                    }
                }
            }
        }
    }
}
```

### Pressure Sensor

The pressure sensor sends the measured pressure constantly.

Calibration

To calibrate your pressure sensor, send an empty CAN message with the message type `MSG_TYPE_CALIBRATION_CMD` , and listen for the  `MSG_TYPE_CALIBRATION_ACK` message with acknowledgement types described below. It is important that the sensor is in atmospheric pressure at the time of calibration, since the calibration will read this as the zero pressure.

Node type: `NODE_TYPE_PRESSURE_SENSOR = 3`

Message types

* To device
  * `MSG_TYPE_NODE_ID = 36`
  * `MSG_TYPE_CALIBRATION_CMD = 28`
* From device
  * `MSG_TYPE_PRESSURE = 13`  $$\[bar]$$
  * `MSG_TYPE_CALIBRATION_ACK = 29`

Acknowledgement types

* `ACK_TYPE_NONE = 0`
* `ACK_TYPE_OK = 2`
* `ACK_TYPE_ERROR = 3`&#x20;

Examples

1. Receive pressure value (bar)

```cpp
#define MAX_NODES 8

float pressure[MAX_NODES] = {0}; // Storage for each node's pressure value

if (can.receive(priority, senderNodeType, receiverNodeType, secondaryNodeId, msgType, subIndex, data, dataLength))
{
    switch (senderNodeType)
    {
        case NODE_TYPE_PRESSURE_SENSOR:
        {
            switch (msgType)
            {
                case MSG_TYPE_PRESSURE:
                {
                    /* Extract sub-index (first byte of data) */
                    subIndex = message.data[0]; // Line not necessary, just here for clarity

                    /* Copy the next "size of a float" bytes to your variable */
                    if (secondaryNodeId < MAX_NODES)
                    {
                        /* Copy incoming float into the correct array index for this node */
                        memcpy(&pressure[secondaryNodeId], &message.data[1], sizeof(float));
                    }
                }
            }
        }
    }
}
```

2. Send calibration command to sensor @ node ID 7 (will read atmospheric pressure and subtract it)

```cpp
int nodeId = 7;
can.sendFloatCan(PRIORITY_HIGH, NODE_TYPE_PLC, NODE_TYPE_PRESSURE_SENSOR,
                 nodeId, MSG_TYPE_CALIBRATION_CMD, 0.0);
```

3. Receive calibration ack (pseudo-code, extend as you like)

```cpp
bool pressureSensorCalibrated = false;

if (can.receive(priority, senderNodeType, receiverNodeType, secondaryNodeId, msgType, subIndex, data, dataLength))
{
    switch (senderNodeType)
    {
        case NODE_TYPE_PRESSURE_SENSOR:
        {
            switch (msgType)
            {
                case MSG_TYPE_CALIBRATION_ACK:
                {
                    /* Extract sub-index (first byte of data) */
                    subIndex = message.data[0]; // Line not necessary, just here for clarity

                    if (secondaryNodeId < MAX_NODES)
                    {
                        const uint32_t ack = 
                        (uint32_t(data[1]) << 24)
                      | (uint32_t(data[2]) << 16)
                      | (uint32_t(data[3]) << 8)
                      |  uint32_t(data[4]);
                                          
                        if(ack == ACK_TYPE_OK)
                        {
                            pressureSensorCalibrated = true;
                        }
                    }
                }
            }
        }
    }
}
```

### Radar Level Sensor

The level sensor sends the measured distance in meters constantly. You can use this value to calculate the volume in your tank.

Node type: `NODE_TYPE_LEVEL_SENSOR = 5`

Message types

* To device
  * `MSG_TYPE_NODE_ID = 36`
  * `MSG_TYPE_MIN = 41`  (Used to set the minimum measure distance of the sensor, usually not necessary to change from default (0.25 m))&#x20;
  * `MSG_TYPE_MAX = 42`  (Used to set the maximum measure distance of the sensor; the sensor can give higher quality measurements if this value is specified. Default is 3.0 m)
* From device
  * `MSG_TYPE_LEVEL = 16`  $$\[m]$$

Examples

1. Send min/max distance value

```cpp
float min = 0.03; // 0.03 m / 3 cm
float max = 1.2; //   1.2 m / 120 cm
int nodeId = 3;
/* Send min value */
can.sendFloatCan(PRIORITY_HIGH, NODE_TYPE_PLC, NODE_TYPE_LEVEL_SENSOR,
                 nodeId, MSG_TYPE_MIN, min);
/* Send max value */
can.sendFloatCan(PRIORITY_HIGH, NODE_TYPE_PLC, NODE_TYPE_LEVEL_SENSOR,
                 nodeId, MSG_TYPE_MAX, max);
```

2. Receive distance from level sensor (m)

```cpp
#define MAX_NODES 8

float levelDistance[MAX_NODES] = {0}; // Storage for each node's distance value

if (can.receive(priority, senderNodeType, receiverNodeType, secondaryNodeId, msgType, subIndex, data, dataLength))
{
    switch (senderNodeType)
    {
        case NODE_TYPE_LEVEL_SENSOR:
        {
            switch (msgType)
            {
                case MSG_TYPE_LEVEL:
                {
                    /* Extract sub-index (first byte of data) */
                    subIndex = message.data[0]; // Line not necessary, just here for clarity

                    /* Copy the next "size of a float" bytes to your variable */
                    if (secondaryNodeId < MAX_NODES)
                    {
                        /* Copy incoming float into the correct array index for this node */
                        memcpy(&levelDistance[secondaryNodeId], &message.data[1], sizeof(float));
                    }
                }
            }
        }
    }
}
```

### Agitator

The agitator sends its measured rounds per minute (RPM) of the motor shaft constantly. It can be controlled by specifying the duty cycle/PWM from 0-100%.

Node type: `NODE_TYPE_AGITATOR = 6`

Message types

* To device
  * `MSG_TYPE_NODE_ID = 36`
  * `MSG_TYPE_PWM = 27`
* From device
  * `MSG_TYPE_RPM = 17`  $$\[RPM]$$

Examples

1. Send agitator PWM/duty cycle value (0-100%)

```cpp
uint32_t agitatorDutyCycle = 50; // 0-100
can.sendUintCan(PRIORITY_HIGH, NODE_TYPE_PLC,
                NODE_TYPE_AGITATOR, 0, MSG_TYPE_PWM, agitatorDutyCycle);
```

2. Receive rounds per minute value from agitator (RPM)

```cpp
#define MAX_NODES 8

float agitatorRPM[MAX_NODES] = {0}; // Storage for each node's RPM value

if (can.receive(priority, senderNodeType, receiverNodeType, secondaryNodeId, msgType, subIndex, data, dataLength))
{
    switch (senderNodeType)
    {
        case NODE_TYPE_AGITATOR:
        {
            switch (msgType)
            {
                case MSG_TYPE_RPM:
                {
                    /* Extract sub-index (first byte of data) */
                    subIndex = message.data[0]; // Line not necessary, just here for clarity

                    if (secondaryNodeId < MAX_NODES)
                    {
                        agitatorRPM[secondaryNodeId] = 
                          (uint32_t(data[1]) << 24)
                        | (uint32_t(data[2]) << 16)
                        | (uint32_t(data[3]) << 8)
                        |  uint32_t(data[4]);
                    }
                }
            }
        }
    }
}
```

### FCS I/O Module

The I/O Module offers the following interfaces:

* 2x PT1000 RTD measurements
* 4x 24V Relay ports
* 4x 24V CAN bus ports
* DC Current measurements for all 8 ports
* External AC relay
* AC measurement for external AC relay
* State feedback for ports and external relay

Node type: `NODE_TYPE_FCS_F_IO = 2`

Message types

* To device
  * `MSG_TYPE_PORT_STATE = 21` (Relay and CAN port states)
  * `MSG_TYPE_POLARITY_STATE = 22` (Relay 4 voltage polarity state)
  * `MSG_TYPE_EXTERNAL_RELAY_STATE = 23` (External relay state)
  * `MSG_TYPE_CAN_TERMINATION = 25`  (CAN bus 120 $$\Omega$$ termination state)
* From device
  * `MSG_TYPE_TEMPERATURE = 12` (PT1000 RTD measurements  $$\[°C]$$)
  * `MSG_TYPE_DCC = 18` (DC Current measurements $$\[A]$$)
  * `MSG_TYPE_ACC = 19` (AC Current measurements $$\[A]$$)
  * `MSG_TYPE_PORT_STATE = 21` (Relay and CAN port states feedback)
  * `MSG_TYPE_POLARITY_STATE = 22` (Relay 4 voltage polarity state feedback)
  * `MSG_TYPE_EXTERNAL_RELAY_STATE = 23` (External relay state feedback)
  * `MSG_TYPE_CAN_TERMINATION = 25`  (CAN bus 120 $$\Omega$$ termination state feedback)

Examples

1. Read temperature measurements

{% hint style="info" %}
Sub index is used to distinguish between different temperature measurements
{% endhint %}

| Sub index | Measurement     |
| --------- | --------------- |
| 0         | Temperature 1   |
| 1         | Temperature 2   |
| 2         | PCB Temperature |

```cpp
float temperature[2] = {0}; // Storage for temperature values

if (can.receive(priority, senderNodeType, receiverNodeType, secondaryNodeId, msgType, subIndex, data, dataLength))
{
    switch (senderNodeType)
    {
        case NODE_TYPE_FCS_F_IO:
        {
            switch (msgType)
            {
                case MSG_TYPE_TEMPERATURE:
                {
                    /* Extract sub-index (first byte of data) */
                    subIndex = message.data[0];

                    /* Copy incoming float into the correct array index */
                    memcpy(&temperature[subIndex], &message.data[1], sizeof(float));
                }
            }
        }
    }
}
```

2. Read DC current measurements

{% hint style="info" %}
Sub index is used to distinguish between different DC current measurements
{% endhint %}

| Sub index | Measurement                     |
| --------- | ------------------------------- |
| 0         | FCS I/O Module total DC current |
| 1         | 6 pin display output DC current |
| 2         | Relay 1 DC current              |
| 3         | Relay 2 DC current              |
| 4         | Relay 3 DC current              |
| 5         | Relay 4 DC current              |
| 6         | CAN 1 DC current                |
| 7         | CAN 2 DC current                |
| 8         | CAN 3 DC current                |
| 9         | CAN 4 DC current                |

```cpp
float dcc[10] = {0}; // Storage for DC current values

if (can.receive(priority, senderNodeType, receiverNodeType, secondaryNodeId, msgType, subIndex, data, dataLength))
{
    switch (senderNodeType)
    {
        case NODE_TYPE_FCS_F_IO:
        {
            switch (msgType)
            {
                case MSG_TYPE_DCC:
                {
                    /* Extract sub-index (first byte of data) */
                    subIndex = message.data[0];

                    /* Copy incoming float into the correct array index */
                    memcpy(&dcc[subIndex], &message.data[1], sizeof(float));
                }
            }
        }
    }
}
```

3. Read AC current measurements

```cpp
float acc = 0; // Storage for AC current values

if (can.receive(priority, senderNodeType, receiverNodeType, secondaryNodeId, msgType, subIndex, data, dataLength))
{
    switch (senderNodeType)
    {
        case NODE_TYPE_FCS_F_IO:
        {
            switch (msgType)
            {
                case MSG_TYPE_ACC:
                {
                    /* Copy incoming float into the correct array index */
                    memcpy(&acc, &message.data[1], sizeof(float));
                }
            }
        }
    }
}
```

4. Send port state command

<pre class="language-cpp"><code class="lang-cpp">/* Variable for keeping track of current port states */
uint8_t portStates;

<strong>/* Helpers for turning on/off port states 1-8 (1-4 relay, 5-8 CAN bus) */
</strong><strong>void turnOnPort(uint8_t &#x26;portStates, uint8_t channel) {
</strong>    if (channel >= 1 &#x26;&#x26; channel &#x3C;= 8) portStates |= (1 &#x3C;&#x3C; (channel - 1));
}
void turnOffPort(uint8_t &#x26;portStates, uint8_t channel) {
    if (channel >= 1 &#x26;&#x26; channel &#x3C;= 8) portStates &#x26;= ~(1 &#x3C;&#x3C; (channel - 1));
}

/* Let's turn on relay 1 and CAN port 2 */
turnOnPort(portStates, 1);
turnOnPort(portstates, 6);

/* Send the command to the I/O Module */
can.sendUintCan(PRIORITY_HIGH, NODE_TYPE_PLC, NODE_TYPE_FCS_F_IO, 0, MSG_TYPE_PORT_STATE, portStates);
</code></pre>
