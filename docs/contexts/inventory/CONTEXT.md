# Inventory

Usable stock of Managed Parts across local and partner sites. It answers who owns stock, who holds it, where it is, and what it may be used for, without claiming more precision than the evidence supports.

## Language

### Stock

**Stock Holding**:
The smallest pool of one Managed Part's stock that can be allocated on its own. Everything in it shares the same owner, custodian, site, state, and use constraints.
_Avoid_: inventory record, lot, bin

**Stock Container**:
Optional physical detail about a carrier of stock, such as a reel, tray, or bag, with its lot and date code.
_Avoid_: package, holding

**Partner Stock Balance**:
A quantity reported by a partner, with its source, as-of time, confidence, and evidence. It carries no container or lot detail the partner did not report.
_Avoid_: partner reel, virtual lot

**Quantity Confidence**:
How certain a quantity is, from an exact count to an estimate such as a partly used reel.
_Avoid_: accuracy, precision

**In-Transit Stock**:
The single position stock occupies between leaving one site and being received at another. It is counted exactly once.
_Avoid_: shipped stock, pending stock

### Who and where

**Recorded Owner**:
The party that owns the stock.
_Avoid_: vendor, supplier, owner company

**Custodian**:
The party that physically holds the stock, which may differ from its Recorded Owner.
_Avoid_: holder, location

**Inventory Site**:
A place where stock is held, such as the team's lab or an assembly house.
_Avoid_: warehouse, location

**Storage Location**:
A specific place within an Inventory Site, such as a shelf or bin.
_Avoid_: site, position

**Stock Source**:
The supplier or origin the stock came from.
_Avoid_: vendor, owner

### Eligibility

**Stock State**:
The condition that decides whether stock can be used right now, such as available, quarantined, or in transit.
_Avoid_: status

**Use Constraint**:
A restriction on which demand stock may satisfy, such as stock reserved by contract for one customer's builds.
_Avoid_: restriction flag, tag

### Changes and allocation

**Stock Movement**:
A recorded change to the quantity, Recorded Owner, Custodian, Inventory Site, or Storage Location of stock. Reservations and floors are never Stock Movements.
_Avoid_: transaction, adjustment

**Named Demand**:
A specific need for parts, such as a number of builds of one KiCad project.
_Avoid_: requirement, order

**Material Reservation**:
A time-bounded claim on stock for one Named Demand. It never changes the quantity on hand.
_Avoid_: allocation, hold

**Stock Floor**:
A standing minimum to keep at an Inventory Site, such as "Keep local". It never changes the quantity on hand.
_Avoid_: safety stock, buffer

**Allocatable Quantity**:
What can be given to one Named Demand after eligibility, other reservations, and protected floors. There is no universal "available" total.
_Avoid_: available, free stock
