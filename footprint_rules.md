# Pad
* Rounded rectangle
* 10% rounding

# Silkscreen
* Layer : F.SilkScreen
* line width : 0.2mm

* only to define the polarity of the component
* limit the use of silkscreen to the minimum necessary to avoid overcrowding the PCB
* keep enough spacing to soldermask opening

# Reference designator
File → Footprint properties → General tab → Reference
* Reference : REF**
* Layer : F.Fab
* Width : 0.5mm
* Height : 0.5mm
* Thickness : 0.1mm

# Courtyard
* Layer : F.Courtyard
* Line width : 0.05mm
* Shape : a simple rectangle

* Draw on a 0.25mm grid (because components are placed on a 0.5mm grid, and the courtyard should be drawn on a 0.25mm grid to ensure proper alignment)

# Component outline
* Layer : F.Fab
* Line width : 0.1mm
* Shape : follow component outline (esp. to mark the polarity of the component or the position of the pin 1)